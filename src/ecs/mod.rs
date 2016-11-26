//! #### Entity Component System (ECS)
//! ECS is an architectural pattern that is widely used in game development. It follows
//! the _Composition_ over _Inheritance_ principle that allows greater flexibility in
//! defining entities where every object in a game's scene in an entity.
//!
//! `Entity` is one of the most fundamental terms in this system. Its basicly some kind
//! of unique identifier to the in-game object. Every `Entity` consists of one or more
//! `Component`s, which define the internal data and how it interacts with the world.
//!
//! Its also common that abstracts `Entity` as container of components, buts with UID
//! approach, we could save the state externaly, users could transfer `Entity` easily
//! without considering the data-ownerships. The real data storage can be shuffled around
//! in memory as needed;
//!
//! #### Data Orinted Design
//! Data-oriented design is a program optimization approach motivated by cache coherency.
//! The approach is to focus on the data layout, separating and sorting fields according
//! to when they are needed, and to think about transformations of data.
//!
//! Due to the composition nature of ECS, its highly compatible with DOD. But benefits
//! doesn't comes for free, there are some memory/performance tradeoff generally. We
//! addressed some data storage approaches in `ecs::component`, users could make their
//! own decision based on different purposes.

#[macro_use]
pub mod component;
pub mod world;

pub use self::component::{Component, ComponentStorage, HashMapStorage};
pub use self::world::World;

use super::utils::handle::*;
pub type Entity = Handle;

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    struct Position {
        x: i32,
        y: i32,
    }

    declare_component!(Position, HashMapStorage);

    #[test]
    fn basic() {
        let mut world = World::new();
        world.register::<Position>();

        let e1 = world.create();
        world.assign::<Position>(e1, Position { x: 1, y: 2 });

        {
            let p = world.fetch::<Position>(e1).unwrap();
            assert_eq!(*p, Position { x: 1, y: 2 });
        }

        {
            let p = world.fetch_mut::<Position>(e1).unwrap();
            p.x = 2;
            p.y = 5;
        }

        {
            let p = world.fetch::<Position>(e1).unwrap();
            assert_eq!(*p, Position { x: 2, y: 5 });
        }

        world.remove::<Position>(e1);
        assert_eq!(world.fetch::<Position>(e1), None);
    }

    #[test]
    fn free() {
        let mut world = World::new();
        world.register::<Position>();

        let e1 = world.create();
        assert!(world.is_alive(e1));
        assert!(!world.has::<Position>(e1));
        assert_eq!(world.fetch::<Position>(e1), None);

        world.assign::<Position>(e1, Position { x: 1, y: 2 });
        assert!(world.has::<Position>(e1));
        world.fetch::<Position>(e1).unwrap();

        world.free(e1);
        assert!(!world.is_alive(e1));
        assert!(!world.has::<Position>(e1));
        assert_eq!(world.fetch::<Position>(e1), None);
    }

    #[test]
    fn duplicated_assign() {
        let mut world = World::new();
        world.register::<Position>();

        let e1 = world.create();
        assert!(world.assign::<Position>(e1, Position { x: 1, y: 2 }) == None);
        assert!(world.assign::<Position>(e1, Position { x: 2, y: 4 }) ==
                Some(Position { x: 1, y: 2 }));

        assert!(world.fetch::<Position>(e1) == Some(&Position { x: 2, y: 4 }))
    }
}