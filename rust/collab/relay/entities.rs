//! The relay's tables as SeaORM entities.

/// `relay_rooms`: a room the relay serves.
pub(super) mod room {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "relay_rooms")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub created_at: i64,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

/// `relay_room_tokens`: the hash of each role's write token for a room.
pub(super) mod token {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "relay_room_tokens")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub room_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub role: String,
        pub token_hash: String,
        pub generation: i64,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

/// `relay_events`: one encrypted blob posted to a room, in order.
pub(super) mod event {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "relay_events")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub room_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub seq: i64,
        pub blob: String,
        pub created_at: i64,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}
