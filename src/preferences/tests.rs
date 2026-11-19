use std::sync::Arc;

use enzui_iced_core::persist::{BackendLoad, PersistBackend, PersistError, Persistable, Store};

use super::{FORMAT_ID, Preferences, VERSION};
use crate::{Message, State};

#[path = "tests/backend_tests.rs"]
mod backend_tests;
#[path = "tests/document_tests.rs"]
mod document_tests;
