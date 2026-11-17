//! Entity definitions and AI behavior systems.

pub mod ai;

pub use ai::{
    DamageEvent, EntityEntry, EntityKind, Goal, GoalFlags, GoalSelector, LookAtPlayerGoal,
    MAX_PENDING_DAMAGE_EVENTS, MeleeAttackGoal, MobEntity, NearestAttackableTargetGoal,
    PrioritizedGoal, RandomStrollGoal, Vec3, WorldView, calculate_look_at, wrap_degrees,
};
