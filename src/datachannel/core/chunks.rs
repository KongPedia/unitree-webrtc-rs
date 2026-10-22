use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

// Resource policy, not firmware limits: bound retained peer data per connection.
const MAX_CHUNKS: usize = 4096;
pub(super) const MAX_TRANSFER_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRANSFERS: usize = 64;
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const TRANSFER_TTL: Duration = Duration::from_secs(30);

struct Transfer {
    schema: &'static str,
    total: usize,
    chunks: BTreeMap<usize, Vec<u8>>,
    bytes: usize,
    deadline: Instant,
}

#[derive(Default)]
pub(super) struct ChunkAssembler {
    transfers: HashMap<String, Transfer>,
}

impl ChunkAssembler {
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }

    pub(super) fn remove(&mut self, key: &str) {
        self.transfers.remove(key);
    }

    pub(super) fn clear(&mut self) {
        self.transfers.clear();
    }

    pub(super) fn expire(&mut self, now: Instant) -> Vec<String> {
        let expired = self
            .transfers
            .iter()
            .filter(|(_, transfer)| transfer.deadline <= now)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in &expired {
            self.remove(key);
        }
        expired
    }

    pub(super) fn insert(
        &mut self,
        key: &str,
        schema: &'static str,
        index: usize,
        total: usize,
        bytes: Vec<u8>,
        now: Instant,
    ) -> Result<Option<Vec<u8>>, String> {
        let result = self.insert_checked(key, schema, index, total, bytes, now);
        if result.is_err() {
            self.remove(key);
        }
        result
    }

    fn insert_checked(
        &mut self,
        key: &str,
        schema: &'static str,
        index: usize,
        total: usize,
        bytes: Vec<u8>,
        now: Instant,
    ) -> Result<Option<Vec<u8>>, String> {
        if total == 0 || total > MAX_CHUNKS || index == 0 || index > total {
            return Err("Invalid chunk count or index".into());
        }
        let retained = self
            .transfers
            .values()
            .map(|transfer| transfer.bytes)
            .sum::<usize>();
        if let Some(transfer) = self.transfers.get(key) {
            if transfer.deadline <= now {
                return Err("Chunk assembly deadline expired".into());
            }
            if transfer.schema != schema {
                return Err("Inconsistent chunk schema".into());
            }
            if transfer.total != total {
                return Err("Inconsistent chunk count".into());
            }
            if let Some(previous) = transfer.chunks.get(&index) {
                return if previous == &bytes {
                    Ok(None)
                } else {
                    Err("Conflicting duplicate chunk".into())
                };
            }
            if bytes.len() > MAX_TRANSFER_BYTES - transfer.bytes {
                return Err("Chunk transfer exceeds byte budget".into());
            }
        } else if self.transfers.len() >= MAX_TRANSFERS || bytes.len() > MAX_TRANSFER_BYTES {
            return Err("Chunk transfer exceeds resource budget".into());
        }
        if bytes.len() > MAX_RETAINED_BYTES - retained {
            return Err("Chunk assembly exceeds retained byte budget".into());
        }
        let transfer = self
            .transfers
            .entry(key.to_owned())
            .or_insert_with(|| Transfer {
                schema,
                total,
                chunks: BTreeMap::new(),
                bytes: 0,
                deadline: now + TRANSFER_TTL,
            });
        transfer.bytes += bytes.len();
        transfer.chunks.insert(index, bytes);
        if transfer.chunks.len() != total {
            return Ok(None);
        }
        let transfer = self
            .transfers
            .remove(key)
            .expect("completed transfer exists");
        Ok(Some(transfer.chunks.into_values().flatten().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_complete_assembly_and_duplicates() {
        let mut assembler = ChunkAssembler::default();
        let now = Instant::now();
        assert_eq!(
            assembler.insert("key", "content", 2, 2, vec![2], now),
            Ok(None)
        );
        assert_eq!(
            assembler.insert("key", "content", 2, 2, vec![2], now),
            Ok(None)
        );
        assert_eq!(
            assembler.insert("key", "content", 1, 2, vec![1], now),
            Ok(Some(vec![1, 2]))
        );
        assert!(assembler.is_empty());
    }

    #[test]
    fn malformed_and_conflicting_chunks_discard_transfer() {
        for (index, total) in [(0, 2), (3, 2), (1, 0), (1, MAX_CHUNKS + 1)] {
            assert!(ChunkAssembler::default()
                .insert("key", "content", index, total, vec![], Instant::now())
                .is_err());
        }
        let mut assembler = ChunkAssembler::default();
        let now = Instant::now();
        assembler
            .insert("key", "content", 1, 2, vec![1], now)
            .unwrap();
        assert!(assembler
            .insert("key", "content", 1, 2, vec![2], now)
            .is_err());
        assert!(assembler.is_empty());
        assembler
            .insert("key", "content", 1, 2, vec![1], now)
            .unwrap();
        assert!(assembler
            .insert("key", "content", 2, 3, vec![2], now)
            .is_err());
        assert!(assembler.is_empty());
    }

    #[test]
    fn schema_mismatch_never_merges_identical_request_keys() {
        let mut assembler = ChunkAssembler::default();
        let now = Instant::now();
        assembler
            .insert("same-id", "content", 1, 2, vec![1], now)
            .unwrap();
        assert!(assembler
            .insert("same-id", "file", 2, 2, vec![2], now)
            .is_err());
        assert!(assembler.is_empty());
    }

    #[test]
    fn expiry_does_not_slide_with_incoming_chunks() {
        let mut assembler = ChunkAssembler::default();
        let now = Instant::now();
        assembler
            .insert("key", "content", 1, 3, vec![1], now)
            .unwrap();
        assembler
            .insert("key", "content", 2, 3, vec![2], now + TRANSFER_TTL / 2)
            .unwrap();
        assert_eq!(assembler.expire(now + TRANSFER_TTL), vec!["key"]);
        assert!(assembler.is_empty());
    }

    #[test]
    fn transfer_count_and_byte_budgets() {
        let mut assembler = ChunkAssembler::default();
        let now = Instant::now();
        for index in 0..MAX_TRANSFERS {
            assembler
                .insert(&index.to_string(), "content", 1, 2, vec![], now)
                .unwrap();
        }
        assert!(assembler
            .insert("extra", "content", 1, 2, vec![], now)
            .is_err());
        assembler.clear();
        assert!(assembler
            .insert(
                "oversize",
                "content",
                1,
                2,
                vec![0; MAX_TRANSFER_BYTES + 1],
                now
            )
            .is_err());
        for index in 0..4 {
            assembler
                .insert(
                    &index.to_string(),
                    "content",
                    1,
                    2,
                    vec![0; MAX_TRANSFER_BYTES],
                    now,
                )
                .unwrap();
        }
        assert!(assembler
            .insert("extra", "content", 1, 2, vec![0], now)
            .is_err());
        assert!(assembler
            .insert("0", "content", 2, 2, vec![0], now)
            .is_err());
    }
}
