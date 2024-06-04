use rockbound::{SchemaBatch, DB};

use alpen_vertex_state::sync_event::SyncEvent;

use crate::errors::DbError;
use crate::{DbResult, traits::SyncEventStore};
use crate::traits::SyncEventProvider;
use super::schemas::{SyncEventSchema, SyncEventWithTimestamp};


pub struct SyncEventDB {
    db: DB
}

impl SyncEventDB {
    // NOTE: db is expected to open all the column families defined in STORE_COLUMN_FAMILIES.
    // FIXME: Make it better/generic.
    pub fn new(db: DB) -> Self {
        Self { db }
    }

    fn get_last_key(&self) -> DbResult<Option<u64>> {
        let mut iterator = self.db.iter::<SyncEventSchema>()?;
        iterator.seek_to_last();
        match iterator.rev().next() {
            Some(res) => {
                let (tip, _) = res?.into_tuple();
                Ok(Some(tip))
            },
            None => Ok(None)
        }
    }
}

impl SyncEventStore for SyncEventDB {
    fn write_sync_event(&self, ev: SyncEvent) -> DbResult<u64> {
        let last_id = self.get_last_key()?.unwrap_or(0);
        let id = last_id + 1;
        let event = SyncEventWithTimestamp::new(ev);
        self.db.put::<SyncEventSchema>(&id, &event)?;
        Ok(id)
    }

    fn clear_sync_event(&self, start_idx: u64, end_idx: u64) -> DbResult<()> {
        if !(start_idx < end_idx) {
            return Err(DbError::Other("start_idx must be less than end_idx".to_string()))
        }

        match self.get_last_key()? {
            Some(last_key) => {
                if !(end_idx <= last_key) {
                    return Err(DbError::Other("end_idx must be less than or equal to last_key".to_string()))
                }
            },
            None => return Err(DbError::Other("cannot clear empty db".to_string()))
        }

        let iterator = self.db.iter::<SyncEventSchema>()?;

        // TODO: determine if the expectation behaviour for this is to clear early events or clear late events
        // The implementation is based for early events
        let mut batch = SchemaBatch::new();

        for res in iterator {
            let (id, _) = res?.into_tuple();
            if id >= end_idx {
                break;
            }

            if id >= start_idx {
                batch.delete::<SyncEventSchema>(&id)?;
            }
        }
        self.db.write_schemas(batch)?;
        Ok(())
    }

}

impl SyncEventProvider for SyncEventDB {
    fn get_last_idx(&self) -> DbResult<Option<u64>> {
        self.get_last_key()
    }

    fn get_sync_event(&self, idx: u64) -> DbResult<Option<SyncEvent>> {
        let event = self.db.get::<SyncEventSchema>(&idx)?;
        match event {
            Some(ev) => Ok(Some(ev.event())),
            None => Ok(None)
        }
    }

    fn get_event_timestamp(&self, idx: u64) -> DbResult<Option<u64>> {
        let event = self.db.get::<SyncEventSchema>(&idx)?;
        match event {
            Some(ev) => Ok(Some(ev.timestamp())),
            None => Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use arbitrary::{Arbitrary, Unstructured};
    use rockbound::schema::ColumnFamilyName;
    use rocksdb::Options;
    use tempfile::TempDir;

    use crate::STORE_COLUMN_FAMILIES;
    use super::*;

    const DB_NAME: &str = "sync_event_db";

    fn generate_arbitrary<'a, T: Arbitrary<'a> + Clone>() -> T {
        let mut u = Unstructured::new(&[1, 2, 3]);
        T::arbitrary(&mut u).expect("failed to generate arbitrary instance")
    }

    fn get_new_db(path: &Path) -> anyhow::Result<DB> {
        // TODO: add other options as appropriate.
        let mut db_opts = Options::default();
        db_opts.create_missing_column_families(true);
        db_opts.create_if_missing(true);
        DB::open(
            path,
            DB_NAME,
            STORE_COLUMN_FAMILIES
                .iter()
                .cloned()
                .collect::<Vec<ColumnFamilyName>>(),
            &db_opts,
        )
    }

    fn setup_db() -> SyncEventDB {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let db = get_new_db(&temp_dir.into_path()).unwrap();
        SyncEventDB::new(db)
    }

    fn insert_event(db: &SyncEventDB) -> SyncEvent {
        let ev: SyncEvent = generate_arbitrary();
        let res = db.write_sync_event(ev.clone());
        assert!(res.is_ok());
        ev
    }

    #[test]
    fn test_get_sync_event() {
        let db = setup_db();

        let ev1 = db.get_sync_event(1).unwrap();
        assert!(ev1.is_none());

        let ev = insert_event(&db);

        let ev1 = db.get_sync_event(1).unwrap();
        assert!(ev1.is_some());

        assert_eq!(ev1.unwrap(), ev);
    }

    #[test]
    fn test_get_last_idx_1() {
        let db = setup_db();

        let idx = db.get_last_idx().unwrap().unwrap_or(0);
        assert_eq!(idx, 0);

        let n = 5;
        for i in 1..=n {
            let _ = insert_event(&db);
            let idx = db.get_last_idx().unwrap().unwrap_or(0);
            assert_eq!(idx, i);
        }
    }

    #[test]
    fn test_get_timestamp() {
        let db = setup_db();
        let mut timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let n = 5;
        for i in 1..=n {
            let _ = insert_event(&db);
            let ts = db.get_event_timestamp(i).unwrap().unwrap();
            assert!(ts >= timestamp);
            timestamp = ts;
        }
    }

    #[test]
    fn test_clear_sync_event() {
        let db = setup_db();
        let n = 5;
        for _ in 1..=n {
            let _ = insert_event(&db);
        }
        // Delete events 2..4
        let res = db.clear_sync_event(2,4);
        assert!(res.is_ok());

        let ev1 = db.get_sync_event(1).unwrap();
        let ev2 = db.get_sync_event(2).unwrap();
        let ev3 = db.get_sync_event(3).unwrap();
        let ev4 = db.get_sync_event(4).unwrap();
        let ev5 = db.get_sync_event(5).unwrap();

        assert!(ev1.is_some());
        assert!(ev2.is_none());
        assert!(ev3.is_none());
        assert!(ev4.is_some());
        assert!(ev5.is_some());
    }

    #[test]
    fn test_clear_sync_event_2() {
        let db = setup_db();
        let n = 5;
        for _ in 1..=n {
            let _ = insert_event(&db);
        }
        let res = db.clear_sync_event(6, 7);
        assert!(res.is_err_and(|x| matches!(x, DbError::Other(ref msg) if msg == "end_idx must be less than or equal to last_key")));
    }


    #[test]
    fn test_get_last_idx_2() {
        let db = setup_db();
        let n = 5;
        for _ in 1..=n {
            let _ = insert_event(&db);
        }
        let res = db.clear_sync_event(2,3);
        assert!(res.is_ok());

        let new_idx = db.get_last_idx().unwrap().unwrap();
        assert_eq!(new_idx, 5);
    }

}