//! Typed load outcomes and bounded tasks.

use super::*;

#[derive(Debug)]
struct RecordingBackend {
    load: BackendLoad,
    published: Arc<std::sync::Mutex<Vec<String>>>,
}

impl PersistBackend for RecordingBackend {
    fn load_raw(&self, _format_id: &str, _version: u32) -> BackendLoad {
        self.load.clone()
    }

    fn save_raw(&self, _format_id: &str, _version: u32, body: &str) -> Result<(), PersistError> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(body.to_owned());
        Ok(())
    }
}

fn recording_store(load: BackendLoad) -> (Store<Preferences>, Arc<std::sync::Mutex<Vec<String>>>) {
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    let store = Store::new(
        RecordingBackend {
            load,
            published: Arc::clone(&published),
        },
        FORMAT_ID,
        VERSION,
    );
    (store, published)
}

#[test]
fn malformed_preferences_preserve_bytes_and_block_writes() {
    let valid = Preferences::persist_default().persist_serialize().unwrap();
    for (body, refused) in [(valid, false), ("policy\tnot-a-policy".to_owned(), true)] {
        let (store, published) = recording_store(BackendLoad::Loaded(body));
        let (mut state, _) = State::boot_for_test_with_preferences(store);
        assert_eq!(state.preferences_load_error.is_some(), refused);
        let _ = state.update(Message::BlockInputEdited("blocked@example.com".into()));
        let task = state.update(Message::AddBlockedSender);
        crate::reducer_tests::apply_task(&mut state, task);
        let writes = published.lock().unwrap();
        assert_eq!(writes.len(), usize::from(!refused));
        if !refused {
            assert!(writes[0].contains("blocked@example.com"));
        }
    }
}
