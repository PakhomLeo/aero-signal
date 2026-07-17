//! 幂等键处理：re-export server-core。

pub use server_core::{
    IdempotencyKey, IdempotencyRecord, IdempotencyStore, InMemoryIdempotencyStore, body_hash,
};
