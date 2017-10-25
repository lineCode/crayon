use std::collections::{HashSet, HashMap};
use std::path::Path;
use std::any::{Any, TypeId};
use std::sync::{Arc, RwLock};
use std::thread;
use std::borrow::Borrow;

use two_lock_queue;
use futures;

use utils::HashValue;
use super::{Resource, ResourceParser, ExternalResourceSystem, ResourceFuture};
use super::arena::ArenaWithCache;
use super::filesystem::{Filesystem, FilesystemDriver};
use super::errors::*;

/// The centralized resource management system.
pub struct ResourceSystem {
    filesystems: Arc<RwLock<FilesystemDriver>>,
    arenas: Arc<RwLock<HashMap<TypeId, ArenaWrapper>>>,
    externs: Arc<RwLock<HashMap<TypeId, ExternSystemWrapper>>>,
    shared: Arc<ResourceSystemShared>,
}

impl ResourceSystem {
    /// Creates a new `ResourceSystem`.
    ///
    /// Notes that this will spawn a worker thread running background to perform
    /// io requests.
    pub fn new() -> Result<Self> {
        let driver = Arc::new(RwLock::new(FilesystemDriver::new()));
        let arenas = Arc::new(RwLock::new(HashMap::new()));
        let externs = Arc::new(RwLock::new(HashMap::new()));

        let (tx, rx) = two_lock_queue::channel(1024);

        {
            let driver = driver.clone();
            let arenas = arenas.clone();
            let externs = externs.clone();

            thread::spawn(|| { ResourceSystem::run(rx, driver, arenas, externs); });
        }

        let shared = ResourceSystemShared::new(driver.clone(), arenas.clone(), tx);

        Ok(ResourceSystem {
               filesystems: driver,
               arenas: arenas,
               externs: externs,
               shared: Arc::new(shared),
           })
    }

    /// Returns the shared parts of `ResourceSystem`.
    pub fn shared(&self) -> Arc<ResourceSystemShared> {
        self.shared.clone()
    }

    /// Registers a new resource type with optional cache.
    #[inline]
    pub fn register<T>(&self, size: usize)
        where T: Resource + Send + Sync + 'static
    {
        let tid = TypeId::of::<T>();
        let mut arenas = self.arenas.write().unwrap();

        if !arenas.contains_key(&tid) {
            let item = ArenaWithCache::<T>::with_capacity(size);
            arenas.insert(tid, ArenaWrapper::new(item));
        }
    }

    ///
    #[inline]
    pub fn register_extern_system<S>(&self, system: S)
        where S: ExternalResourceSystem + Send + Sync + 'static
    {
        let tid = TypeId::of::<S>();
        let mut externs = self.externs.write().unwrap();

        if !externs.contains_key(&tid) {
            externs.insert(tid, ExternSystemWrapper::new(system));
        }
    }

    /// Mount a file-system drive with identifier.
    #[inline]
    pub fn mount<S, F>(&self, ident: S, fs: F) -> Result<()>
        where S: Borrow<str>,
              F: Filesystem + 'static
    {
        self.filesystems.write().unwrap().mount(ident, fs)
    }

    /// Unmount a file-system from this collection.
    #[inline]
    pub fn unmount<S>(&self, ident: S)
        where S: Borrow<str>
    {
        self.filesystems.write().unwrap().unmount(ident);
    }

    ///
    pub fn advance(&self) -> Result<()> {
        {
            let mut arenas = self.arenas.write().unwrap();
            for (_, v) in arenas.iter_mut() {
                v.unload_unused();
            }
        }

        {
            let mut externs = self.externs.write().unwrap();
            for (_, v) in externs.iter_mut() {
                v.unload_unused();
            }
        }

        Ok(())
    }

    fn run(chan: two_lock_queue::Receiver<ResourceTask>,
           driver: Arc<RwLock<FilesystemDriver>>,
           arenas: Arc<RwLock<HashMap<TypeId, ArenaWrapper>>>,
           externs: Arc<RwLock<HashMap<TypeId, ExternSystemWrapper>>>) {
        let mut locks: HashSet<HashValue<Path>> = HashSet::new();
        let mut buf = Vec::new();

        loop {
            match chan.recv().unwrap() {
                ResourceTask::Load { mut closure } => {
                    let driver = driver.read().unwrap();
                    closure(&arenas, &driver, &mut locks, &mut buf);
                }

                ResourceTask::ExternLoad { mut closure } => {
                    let driver = driver.read().unwrap();
                    closure(&arenas, &externs, &driver, &mut locks, &mut buf);
                }

                ResourceTask::UnloadUnused => {
                    let mut arenas = arenas.write().unwrap();
                    for (_, v) in arenas.iter_mut() {
                        v.unload_unused();
                    }
                }

                ResourceTask::Stop => return,
            }
        }
    }

    #[inline]
    fn cast_extern<S>(system: &mut Any) -> &mut S
        where S: ExternalResourceSystem + 'static
    {
        system.downcast_mut::<S>().unwrap()
    }

    fn load_extern<S>(externs: &RwLock<HashMap<TypeId, ExternSystemWrapper>>,
                      path: &Path,
                      src: &S::Data,
                      options: S::Options)
                      -> Result<Arc<S::Item>>
        where S: ExternalResourceSystem + 'static
    {
        let tid = TypeId::of::<S>();
        let mut externs = externs.write().unwrap();
        let wrapper = externs.get_mut(&tid).ok_or(ErrorKind::NotRegistered)?;
        ResourceSystem::cast_extern::<S>(wrapper.system.as_mut()).load(path, src, options)
    }

    #[inline]
    fn cast<T>(arena: &mut Any) -> &mut ArenaWithCache<T::Item>
        where T: ResourceParser
    {
        arena.downcast_mut::<ArenaWithCache<T::Item>>().unwrap()
    }

    fn load<T>(path: &Path,
               arenas: &RwLock<HashMap<TypeId, ArenaWrapper>>,
               driver: &FilesystemDriver,
               locks: &mut HashSet<HashValue<Path>>,
               buf: &mut Vec<u8>)
               -> Result<Arc<T::Item>>
        where T: ResourceParser
    {
        let hash = (&path).into();
        let tid = TypeId::of::<T::Item>();

        {
            let mut arenas = arenas.write().unwrap();
            let v = arenas.get_mut(&tid).ok_or(ErrorKind::NotRegistered)?;
            if let Some(rc) = ResourceSystem::cast::<T>(v.arena.as_mut()).get(hash) {
                return Ok(rc);
            }
        }

        if locks.contains(&hash) {
            bail!(ErrorKind::CircularReferenceFound);
        }

        let rc = {
            locks.insert(hash);
            let from = buf.len();
            driver.load_into(&path, buf)?;
            let resource = T::parse(&buf[from..])?;
            locks.remove(&hash);
            Arc::new(resource)
        };

        {
            let mut arenas = arenas.write().unwrap();
            let v = arenas.get_mut(&tid).ok_or(ErrorKind::NotRegistered)?;
            ResourceSystem::cast::<T>(v.arena.as_mut()).insert(hash, rc.clone());
        }

        Ok(rc)
    }
}

pub struct ResourceSystemShared {
    filesystems: Arc<RwLock<FilesystemDriver>>,
    arenas: Arc<RwLock<HashMap<TypeId, ArenaWrapper>>>,
    chan: two_lock_queue::Sender<ResourceTask>,
}

enum ResourceTask {
    Load {
        closure: Box<FnMut(&RwLock<HashMap<TypeId, ArenaWrapper>>,
                           &FilesystemDriver,
                           &mut HashSet<HashValue<Path>>,
                           &mut Vec<u8>) + Send + Sync>,
    },
    ExternLoad {
        closure: Box<FnMut(&RwLock<HashMap<TypeId, ArenaWrapper>>,
                           &RwLock<HashMap<TypeId, ExternSystemWrapper>>,
                           &FilesystemDriver,
                           &mut HashSet<HashValue<Path>>,
                           &mut Vec<u8>) + Send + Sync>,
    },
    UnloadUnused,
    Stop,
}

impl ResourceSystemShared {
    fn new(filesystems: Arc<RwLock<FilesystemDriver>>,
           arenas: Arc<RwLock<HashMap<TypeId, ArenaWrapper>>>,
           chan: two_lock_queue::Sender<ResourceTask>)
           -> Self {
        ResourceSystemShared {
            filesystems: filesystems,
            arenas: arenas,
            chan: chan,
        }
    }

    pub fn exists<T, P>(&self, path: P) -> bool
        where P: AsRef<Path>
    {
        self.filesystems.read().unwrap().exists(path)
    }

    pub fn load_extern<T, S, P>(&self, path: P, options: S::Options) -> ResourceFuture<S::Item>
        where T: ResourceParser,
              S: ExternalResourceSystem<Data = T::Item> + 'static,
              P: AsRef<Path>
    {
        let (tx, rx) = futures::sync::oneshot::channel();

        // Hacks: Optimize this when Box<FnOnce> is usable.
        let path = path.as_ref().to_owned();
        let payload = Arc::new(RwLock::new(Some((path, tx, options))));
        let closure = move |a: &RwLock<HashMap<TypeId, ArenaWrapper>>,
                            e: &RwLock<HashMap<TypeId, ExternSystemWrapper>>,
                            d: &FilesystemDriver,
                            l: &mut HashSet<HashValue<Path>>,
                            b: &mut Vec<u8>| {
            if let Some(data) = payload.write().unwrap().take() {
                let v =
                    ResourceSystem::load::<T>(&data.0, a, d, l, b)
                        .and_then(|src| ResourceSystem::load_extern::<S>(e, &data.0, &src, data.2));
                data.1.send(v).is_ok();
            }
        };

        self.chan
            .send(ResourceTask::ExternLoad { closure: Box::new(closure) })
            .unwrap();

        ResourceFuture(rx)
    }

    pub fn load<T, P>(&self, path: P) -> ResourceFuture<T::Item>
        where T: ResourceParser,
              P: AsRef<Path>
    {
        let (tx, rx) = futures::sync::oneshot::channel();
        let hash: HashValue<Path> = path.as_ref().into();
        let tid = TypeId::of::<T::Item>();

        {
            // Returns directly if we have this resource in memory.
            let mut arenas = self.arenas.write().unwrap();
            if let Some(v) = arenas.get_mut(&tid) {
                if let Some(rc) = ResourceSystem::cast::<T>(v.arena.as_mut()).get(hash) {
                    tx.send(Ok(rc)).is_ok();
                    return ResourceFuture(rx);
                }
            }
        }

        // Hacks: Optimize this when Box<FnOnce> is usable.
        let path = path.as_ref().to_owned();
        let payload = Arc::new(RwLock::new(Some((path, tx))));
        let closure = move |a: &RwLock<HashMap<TypeId, ArenaWrapper>>,
                            d: &FilesystemDriver,
                            l: &mut HashSet<HashValue<Path>>,
                            b: &mut Vec<u8>| {
            if let Some(data) = payload.write().unwrap().take() {
                let v = ResourceSystem::load::<T>(&data.0, a, d, l, b);
                data.1.send(v).is_ok();
            }
        };

        self.chan
            .send(ResourceTask::Load { closure: Box::new(closure) })
            .unwrap();

        ResourceFuture(rx)
    }

    /// Unload unused resources from memory.
    pub fn unload_unused(&self) {
        self.chan.send(ResourceTask::UnloadUnused).unwrap();
    }
}

impl Drop for ResourceSystemShared {
    fn drop(&mut self) {
        self.chan.send(ResourceTask::Stop).unwrap();
    }
}

/// Anonymous operations helper.
struct ArenaWrapper {
    arena: Box<Any + Send + Sync>,
    unload_unused: Box<FnMut(&mut Any) + Send + Sync>,
}

impl ArenaWrapper {
    fn new<T>(item: ArenaWithCache<T>) -> Self
        where T: Resource + Send + Sync + 'static
    {
        let unload_unused = move |a: &mut Any| {
            let a = a.downcast_mut::<ArenaWithCache<T>>().unwrap();
            a.unload_unused();
        };

        ArenaWrapper {
            arena: Box::new(item),
            unload_unused: Box::new(unload_unused),
        }
    }

    #[inline]
    fn unload_unused(&mut self) {
        (self.unload_unused)(self.arena.as_mut())
    }
}

struct ExternSystemWrapper {
    system: Box<Any + Send + Sync>,
    unload_unused: Box<FnMut(&mut Any) + Send + Sync>,
}

impl ExternSystemWrapper {
    fn new<T>(item: T) -> Self
        where T: ExternalResourceSystem + Send + Sync + 'static
    {
        let unload_unused = move |a: &mut Any| {
            let a = a.downcast_mut::<T>().unwrap();
            a.unload_unused();
        };

        ExternSystemWrapper {
            system: Box::new(item),
            unload_unused: Box::new(unload_unused),
        }
    }

    #[inline]
    fn unload_unused(&mut self) {
        (self.unload_unused)(self.system.as_mut())
    }
}