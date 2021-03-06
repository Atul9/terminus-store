//! a synchronous version of the store API
//!
//! Since not everyone likes tokio, or dealing with async code, this
//! module exposes the same API as the asynchronous store API, only
//! without any futures.
use tokio::runtime::Runtime;
use futures::prelude::*;
use futures::future;
use futures::sync::oneshot;

use std::io;
use std::path::PathBuf;

use crate::layer::{Layer,ObjectType,StringTriple,IdTriple,SubjectLookup,ObjectLookup, PredicateLookup};
use crate::store::{Store, NamedGraph, StoreLayer, StoreLayerBuilder, open_memory_store, open_directory_store};

lazy_static! {
    static ref RUNTIME: Runtime = Runtime::new().unwrap();
}

/// Trampoline function for calling the async api in a sync way.
///
/// This convoluted mess was implemented because doing oneshot::spawn
/// directly on the async api functions resulted in a memory leak in
/// tokio_threadpool. Spawning the future indirectly appears to work
/// without memory leak.
fn task_sync<T:'static+Send,F:'static+Future<Item=T,Error=io::Error>+Send>(future: F) -> Result<T,io::Error> {
    let (tx, rx) = oneshot::channel();
    let wrapped_future = future::lazy(|| {
        tokio::spawn(future.then(|r|tx.send(r)).map(|_|()).map_err(|_|()));
        future::ok::<(),io::Error>(())
    });

    let receiver_future = rx
        .map_err(|_|io::Error::new(io::ErrorKind::Other, "canceled"))
        .and_then(|r|r);

    oneshot::spawn(wrapped_future.and_then(|_|receiver_future), &RUNTIME.executor()).wait()
}

/// A wrapper over a SimpleLayerBuilder, providing a thread-safe sharable interface
///
/// The SimpleLayerBuilder requires one to have a mutable reference to
/// it, and on commit it will be consumed. This builder only requires
/// an immutable reference, and uses a futures-aware read-write lock
/// to synchronize access to it between threads. Also, rather than
/// consuming itself on commit, this wrapper will simply mark itself
/// as having committed, returning errors on further calls.
pub struct SyncStoreLayerBuilder {
    inner: StoreLayerBuilder,
}

impl SyncStoreLayerBuilder {
    fn wrap(inner: StoreLayerBuilder) -> Self {
        SyncStoreLayerBuilder {
            inner
        }
    }

    /// Returns the name of the layer being built
    pub fn name(&self) -> [u32;5] {
        self.inner.name()
    }

    /// Add a string triple
    pub fn add_string_triple(&self, triple: &StringTriple) -> Result<(), io::Error> {
        task_sync(self.inner.add_string_triple(triple))
    }

    /// Add an id triple
    pub fn add_id_triple(&self, triple: IdTriple) -> Result<bool, io::Error> {
        task_sync(self.inner.add_id_triple(triple))
    }

    /// Remove a string triple
    pub fn remove_string_triple(&self, triple: &StringTriple) -> Result<bool, io::Error> {
        task_sync(self.inner.remove_string_triple(triple))
    }

    /// Remove an id triple
    pub fn remove_id_triple(&self, triple: IdTriple) -> Result<bool, io::Error> {
        task_sync(self.inner.remove_id_triple(triple))
    }

    /// Commit the layer to storage
    pub fn commit(&self) -> Result<SyncStoreLayer,io::Error> {
        let inner = task_sync(self.inner.commit());

        inner.map(|i|SyncStoreLayer::wrap(i))
    }
}

/// A layer that keeps track of the store it came out of, allowing the creation of a layer builder on top of this layer
#[derive(Clone)]
pub struct SyncStoreLayer {
    inner: StoreLayer,
}

impl SyncStoreLayer {
    fn wrap(inner: StoreLayer) -> Self {
        Self {
            inner
        }
    }

    /// Create a layer builder based on this layer
    pub fn open_write(&self) -> Result<SyncStoreLayerBuilder,io::Error> {
        let inner = task_sync(self.inner.open_write());

        inner.map(|i|SyncStoreLayerBuilder::wrap(i))
    }

    pub fn parent(&self) -> Option<SyncStoreLayer> {
        self.inner.parent()
            .map(|p| SyncStoreLayer {
                inner: p
            })
    }
}

impl Layer for SyncStoreLayer {
    fn name(&self) -> [u32;5] {
        self.inner.name()
    }

    fn parent(&self) -> Option<&dyn Layer> {
        (&self.inner as &dyn Layer).parent()
    }

    fn node_dict_id(&self, subject: &str) -> Option<u64> {
        self.inner.node_dict_id(subject)
    }

    fn value_dict_id(&self, value: &str) -> Option<u64> {
        self.inner.value_dict_id(value)
    }

    fn node_dict_len(&self) -> usize {
        self.inner.node_dict_len()
    }

    fn value_dict_len(&self) -> usize {
        self.inner.value_dict_len()
    }

    fn value_dict_get(&self, id: usize) -> String {
        self.inner.value_dict_get(id)
    }

    fn node_dict_get(&self, id: usize) -> String {
        self.inner.node_dict_get(id)
    }

    fn node_and_value_count(&self) -> usize {
        self.inner.node_and_value_count()
    }

    fn predicate_dict_id(&self, predicate: &str) -> Option<u64> {
        self.inner.predicate_dict_id(predicate)
    }

    fn predicate_dict_get(&self, id: usize) -> String {
        self.inner.predicate_dict_get(id)
    }

    fn predicate_dict_len(&self) -> usize {
        self.inner.predicate_dict_len()
    }

    fn predicate_count(&self) -> usize {
        self.inner.predicate_count()
    }

    fn subject_id(&self, subject: &str) -> Option<u64> {
        self.inner.subject_id(subject)
    }

    fn predicate_id(&self, predicate: &str) -> Option<u64> {
        self.inner.predicate_id(predicate)
    }

    fn object_node_id(&self, object: &str) -> Option<u64> {
        self.inner.object_node_id(object)
    }

    fn object_value_id(&self, object: &str) -> Option<u64> {
        self.inner.object_value_id(object)
    }

    fn id_subject(&self, id: u64) -> Option<String> {
        self.inner.id_subject(id)
    }

    fn id_predicate(&self, id: u64) -> Option<String> {
        self.inner.id_predicate(id)
    }

    fn id_object(&self, id: u64) -> Option<ObjectType> {
        self.inner.id_object(id)
    }

    fn subjects(&self) -> Box<dyn Iterator<Item=Box<dyn SubjectLookup>>> {
        self.inner.subjects()
    }

    fn subject_additions(&self) -> Box<dyn Iterator<Item=Box<dyn SubjectLookup>>> {
        self.inner.subject_additions()
    }

    fn subject_removals(&self) -> Box<dyn Iterator<Item=Box<dyn SubjectLookup>>> {
        self.inner.subject_removals()
    }

    fn lookup_subject(&self, subject: u64) -> Option<Box<dyn SubjectLookup>> {
        self.inner.lookup_subject(subject)
    }

    fn lookup_subject_addition(&self, subject: u64) -> Option<Box<dyn SubjectLookup>> {
        self.inner.lookup_subject_addition(subject)
    }

    fn lookup_subject_removal(&self, subject: u64) -> Option<Box<dyn SubjectLookup>> {
        self.inner.lookup_subject_removal(subject)
    }

    fn objects(&self) -> Box<dyn Iterator<Item=Box<dyn ObjectLookup>>> {
        self.inner.objects()
    }

    fn object_additions(&self) -> Box<dyn Iterator<Item=Box<dyn ObjectLookup>>> {
        self.inner.object_additions()
    }

    fn object_removals(&self) -> Box<dyn Iterator<Item=Box<dyn ObjectLookup>>> {
        self.inner.object_removals()
    }

    fn lookup_object(&self, object: u64) -> Option<Box<dyn ObjectLookup>> {
        self.inner.lookup_object(object)
    }

    fn lookup_object_addition(&self, object: u64) -> Option<Box<dyn ObjectLookup>> {
        self.inner.lookup_object_addition(object)
    }

    fn lookup_object_removal(&self, object: u64) -> Option<Box<dyn ObjectLookup>> {
        self.inner.lookup_object_removal(object)
    }

    fn lookup_predicate(&self, predicate: u64) -> Option<Box<dyn PredicateLookup>> {
        self.inner.lookup_predicate(predicate)
    }

    fn lookup_predicate_addition(&self, predicate: u64) -> Option<Box<dyn PredicateLookup>> {
        self.inner.lookup_predicate_addition(predicate)
    }

    fn lookup_predicate_removal(&self, predicate: u64) -> Option<Box<dyn PredicateLookup>> {
        self.inner.lookup_predicate_removal(predicate)
    }

    fn lookup_subject_current_layer(&self, subject: u64, parent: Option<Box<dyn SubjectLookup>>) -> Option<Box<dyn SubjectLookup>> {
        self.inner.lookup_subject_current_layer(subject, parent)
    }

    fn lookup_predicate_current_layer(&self, predicate: u64, parent: Option<Box<dyn PredicateLookup>>) -> Option<Box<dyn PredicateLookup>> {
        self.inner.lookup_predicate_current_layer(predicate, parent)
    }

    fn lookup_object_current_layer(&self, object: u64, parent: Option<Box<dyn ObjectLookup>>) -> Option<Box<dyn ObjectLookup>> {
        self.inner.lookup_object_current_layer(object, parent)
    }

    fn clone_boxed(&self) -> Box<dyn Layer> {
        Box::new(self.clone())
    }
}

/// A named graph in terminus-store.
///
/// Named graphs in terminus-store are basically just a label pointing
/// to a layer. Opening a read transaction to a named graph is just
/// getting hold of the layer it points at, as layers are
/// read-only. Writing to a named graph is just making it point to a
/// new layer.
pub struct SyncNamedGraph {
    inner: NamedGraph,
}

impl SyncNamedGraph {
    fn wrap(inner: NamedGraph) -> Self {
        Self {
            inner
        }
    }

    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// Returns the layer this database points at
    pub fn head(&self) -> Result<Option<SyncStoreLayer>,io::Error> {
        let inner = task_sync(self.inner.head());

        inner.map(|i|i.map(|i|SyncStoreLayer::wrap(i)))
    }

    /// Set the database label to the given layer if it is a valid ancestor, returning false otherwise
    pub fn set_head(&self, layer: &SyncStoreLayer) -> Result<bool, io::Error> {
        task_sync(self.inner.set_head(&layer.inner))
    }
}

/// A store, storing a set of layers and database labels pointing to these layers
pub struct SyncStore {
    inner: Store
}

impl SyncStore {
    /// wrap an asynchronous `Store`, running all futures on a lazily-constructed tokio runtime
    ///
    /// The runtime will be constructed on the first call to wrap. Any
    /// subsequent SyncStore will reuse the same runtime.
    pub fn wrap(inner: Store) -> Self {
        Self {
            inner,
        }
    }

    /// Create a new database with the given name
    ///
    /// If the database already exists, this will return an error
    pub fn create(&self, label: &str) -> Result<SyncNamedGraph,io::Error> {
        let inner = task_sync(self.inner.create(label));

        inner.map(|i| SyncNamedGraph::wrap(i))
    }

    /// Open an existing database with the given name, or None if it does not exist
    pub fn open(&self, label: &str) -> Result<Option<SyncNamedGraph>,io::Error> {
        let inner = task_sync(self.inner.open(label));

        inner.map(|i| i.map(|i|SyncNamedGraph::wrap(i)))
    }

    /// Create a base layer builder, unattached to any database label
    ///
    /// After having committed it, use `set_head` on a `NamedGraph` to attach it.
    pub fn create_base_layer(&self) -> Result<SyncStoreLayerBuilder,io::Error> {
        let inner = task_sync(self.inner.create_base_layer());

        inner.map(|i| SyncStoreLayerBuilder::wrap(i))
    }
}

/// Open a store that is entirely in memory
///
/// This is useful for testing purposes, or if the database is only going to be used for caching purposes
pub fn open_sync_memory_store() -> SyncStore {
    SyncStore::wrap(open_memory_store())
}

/// Open a store that stores its data in the given directory
pub fn open_sync_directory_store<P:Into<PathBuf>>(path: P) -> SyncStore {
    SyncStore::wrap(open_directory_store(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_and_manipulate_sync_memory_database() {
        let store = open_sync_memory_store();
        let database = store.create("foodb").unwrap();

        let head = database.head().unwrap();
        assert!(head.is_none());

        let mut builder = store.create_base_layer().unwrap();
        builder.add_string_triple(&StringTriple::new_value("cow","says","moo")).unwrap();

        let layer = builder.commit().unwrap();
        assert!(database.set_head(&layer).unwrap());

        builder = layer.open_write().unwrap();
        builder.add_string_triple(&StringTriple::new_value("pig","says","oink")).unwrap();

        let layer2 = builder.commit().unwrap();
        assert!(database.set_head(&layer2).unwrap());
        let layer2_name = layer2.name();

        let layer = database.head().unwrap().unwrap();

        assert_eq!(layer2_name, layer.name());
        assert!(layer.string_triple_exists(&StringTriple::new_value("cow","says","moo")));
        assert!(layer.string_triple_exists(&StringTriple::new_value("pig","says","oink")));
    }

    #[test]
    fn create_and_manipulate_sync_directory_database() {
        let dir = tempdir().unwrap();
        let store = open_sync_directory_store(dir.path());
        let database = store.create("foodb").unwrap();

        let head = database.head().unwrap();
        assert!(head.is_none());

        let mut builder = store.create_base_layer().unwrap();
        builder.add_string_triple(&StringTriple::new_value("cow","says","moo")).unwrap();

        let layer = builder.commit().unwrap();
        assert!(database.set_head(&layer).unwrap());

        builder = layer.open_write().unwrap();
        builder.add_string_triple(&StringTriple::new_value("pig","says","oink")).unwrap();

        let layer2 = builder.commit().unwrap();
        assert!(database.set_head(&layer2).unwrap());
        let layer2_name = layer2.name();

        let layer = database.head().unwrap().unwrap();

        assert_eq!(layer2_name, layer.name());
        assert!(layer.string_triple_exists(&StringTriple::new_value("cow","says","moo")));
        assert!(layer.string_triple_exists(&StringTriple::new_value("pig","says","oink")));
    }
}
