//! In-memory implementation of storage traits.
use futures::prelude::*;
use tokio::prelude::*;
use std::sync::{self,Arc};
use futures_locks;
use std::io;
use std::collections::HashMap;

use super::*;
use crate::layer::{Layer, LayerBuilder, BaseLayer, ChildLayer, SimpleLayerBuilder};

pub struct MemoryBackedStoreWriter {
    vec: Arc<sync::RwLock<Vec<u8>>>,
    pos: usize
}

impl Write for MemoryBackedStoreWriter {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        let mut v = self.vec.write().unwrap();
        if v.len() - self.pos < buf.len() {
            v.resize(self.pos + buf.len(), 0);
        }

        v[self.pos..self.pos+buf.len()].copy_from_slice(buf);

        self.pos += buf.len();

        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

impl AsyncWrite for MemoryBackedStoreWriter {
    fn shutdown(&mut self) -> Result<Async<()>, io::Error> {
        Ok(Async::Ready(()))
    }
}

pub struct MemoryBackedStoreReader {
    vec: Arc<sync::RwLock<Vec<u8>>>,
    pos: usize
}

impl Read for MemoryBackedStoreReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let v = self.vec.read().unwrap();

        if self.pos >= v.len() {
            return Ok(0);
        }

        let slice = &v[self.pos..];
        if slice.len() >= buf.len() {
            buf.copy_from_slice(&slice[..buf.len()]);
            self.pos += buf.len();

            Ok(buf.len())
        }
        else {
            buf[..slice.len()].copy_from_slice(slice);
            self.pos += slice.len();

            Ok(slice.len())
        }
    }
}

impl AsyncRead for MemoryBackedStoreReader {
}

#[derive(Clone,Debug)]
pub struct SharedVec(pub Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedVec {
    fn as_ref(&self) -> &[u8] {
        &*self.0
    }
}

#[derive(Clone)]
pub struct MemoryBackedStore {
    vec: Arc<sync::RwLock<Vec<u8>>>
}

impl MemoryBackedStore {
    pub fn new() -> MemoryBackedStore {
        MemoryBackedStore { vec: Default::default() }
    }
}

impl FileStore for MemoryBackedStore {
    type Write = MemoryBackedStoreWriter;

    fn open_write_from(&self, pos: usize) -> MemoryBackedStoreWriter {
        MemoryBackedStoreWriter { vec: self.vec.clone(), pos }
    }
}

impl FileLoad for MemoryBackedStore {
    type Read = MemoryBackedStoreReader;
    type Map = SharedVec;

    fn size(&self) -> usize {
        self.vec.read().unwrap().len()
    }

    fn open_read_from(&self, offset: usize) -> MemoryBackedStoreReader {
        MemoryBackedStoreReader { vec: self.vec.clone(), pos: offset }
    }

    fn map(&self) -> Box<dyn Future<Item=SharedVec,Error=std::io::Error>+Send> {
        let vec = self.vec.clone();
        Box::new(future::lazy(move ||future::ok(SharedVec(Arc::new(vec.read().unwrap().clone())))))
    }
}

#[derive(Clone)]
pub struct MemoryLayerStore {
    layers: futures_locks::RwLock<HashMap<[u32;5],(Option<[u32;5]>,LayerFiles<MemoryBackedStore>)>>
}

impl MemoryLayerStore {
    pub fn new() -> MemoryLayerStore {
        MemoryLayerStore {
            layers: futures_locks::RwLock::new(HashMap::new())
        }
    }
}

impl LayerRetriever for MemoryLayerStore {
    fn layers(&self) -> Box<dyn Future<Item=Vec<[u32;5]>, Error=io::Error>+Send> {
        Box::new(self.layers.read()
                 .then(|layers|Ok(layers.expect("rwlock read cannot fail").keys().map(|k|k.clone()).collect())))
    }

    fn get_layer_with_retriever(&self, name: [u32;5], retriever: Box<dyn LayerRetriever>) -> Box<dyn Future<Item=Option<Arc<dyn Layer>>,Error=io::Error>+Send> {
        Box::new(self.layers.read()
                 .then(move |layers| {
                     let layers = layers.expect("rwlock read should always succeed");
                     let saved = layers.get(&name).map(|x|x.clone());
                     let fut: Box<dyn Future<Item=_,Error=_>+Send> = match saved {
                         None => Box::new(future::ok(None)),
                         Some(saved) => Box::new(
                             future::ok(saved)
                                 .and_then(move |(parent_name, files)| {
                                     let fut: Box<dyn Future<Item=_,Error=_>+Send> =
                                         if parent_name.is_some() {
                                             let files = files.clone().into_child();
                                             Box::new(retriever.get_layer(parent_name.unwrap())
                                                      .and_then(|parent| match parent {
                                                          None => Err(io::Error::new(io::ErrorKind::InvalidData, "expected parent layer to exist")),
                                                          Some(p) => Ok(p)
                                                      })
                                                      .and_then(move |parent| ChildLayer::load_from_files(name, parent, &files))
                                                      .map(|layer| Some(Arc::new(layer) as Arc<dyn Layer>)))
                                         } else {
                                             Box::new(BaseLayer::load_from_files(name, &files.clone().into_base())
                                                      .map(|layer| Some(Arc::new(layer) as Arc<dyn Layer>)))
                                         };
                                     fut
                                 }))
                     };

                     fut
                 }))

    }

    fn boxed_retriever(&self) -> Box<dyn LayerRetriever> {
        Box::new(self.clone())
    }
}

impl LayerStore for MemoryLayerStore {
    fn create_base_layer(&self) -> Box<dyn Future<Item=Box<dyn LayerBuilder>,Error=io::Error>+Send> {
        let name = rand::random();

        let files: Vec<_> = (0..21).map(|_| MemoryBackedStore::new()).collect();
        let blf = BaseLayerFiles {
            node_dictionary_files: DictionaryFiles {
                blocks_file: files[0].clone(),
                offsets_file: files[1].clone()
            },
            predicate_dictionary_files: DictionaryFiles {
                blocks_file: files[2].clone(),
                offsets_file: files[3].clone()
            },
            value_dictionary_files: DictionaryFiles {
                blocks_file: files[4].clone(),
                offsets_file: files[5].clone()
            },
            s_p_adjacency_list_files: AdjacencyListFiles {
                bitindex_files: BitIndexFiles {
                    bits_file: files[6].clone(),
                    blocks_file: files[7].clone(),
                    sblocks_file: files[8].clone(),
                },
                nums_file: files[9].clone()
            },
            sp_o_adjacency_list_files: AdjacencyListFiles {
                bitindex_files: BitIndexFiles {
                    bits_file: files[10].clone(),
                    blocks_file: files[11].clone(),
                    sblocks_file: files[12].clone(),
                },
                nums_file: files[13].clone()
            },
            o_ps_adjacency_list_files: AdjacencyListFiles {
                bitindex_files: BitIndexFiles {
                    bits_file: files[14].clone(),
                    blocks_file: files[15].clone(),
                    sblocks_file: files[16].clone(),
                },
                nums_file: files[17].clone()
            },
            predicate_wavelet_tree_files: BitIndexFiles {
                bits_file: files[18].clone(),
                blocks_file: files[19].clone(),
                sblocks_file: files[20].clone(),
            },
        };

        Box::new(self.layers.write()
                 .then(move |layers| {
                     layers.expect("rwlock write should always succeed").insert(name, (None, LayerFiles::Base(blf.clone())));
                     Ok(Box::new(SimpleLayerBuilder::new(name, blf)) as Box<dyn LayerBuilder>)
                 }))
    }

    fn create_child_layer_with_retriever(&self, parent: [u32;5], retriever: Box<dyn LayerRetriever>) -> Box<dyn Future<Item=Box<dyn LayerBuilder>,Error=io::Error>+Send> {
        let layers = self.layers.clone();
        Box::new(retriever.get_layer(parent)
                 .and_then(|parent_layer| match parent_layer {
                     None => future::err(io::Error::new(io::ErrorKind::NotFound, "parent layer not found")),
                     Some(parent_layer) => future::ok(parent_layer)
                 })
                 .and_then(move |parent_layer| {
                     let name = rand::random();
                     let files: Vec<_> = (0..40).map(|_| MemoryBackedStore::new()).collect();

                     let clf = ChildLayerFiles {
                         node_dictionary_files: DictionaryFiles {
                             blocks_file: files[0].clone(),
                             offsets_file: files[1].clone()
                         },
                         predicate_dictionary_files: DictionaryFiles {
                             blocks_file: files[2].clone(),
                             offsets_file: files[3].clone()
                         },
                         value_dictionary_files: DictionaryFiles {
                             blocks_file: files[4].clone(),
                             offsets_file: files[5].clone()
                         },

                         pos_subjects_file: files[6].clone(),
                         pos_objects_file: files[7].clone(),
                         neg_subjects_file: files[8].clone(),
                         neg_objects_file: files[9].clone(),

                         pos_s_p_adjacency_list_files: AdjacencyListFiles {
                             bitindex_files: BitIndexFiles {
                                 bits_file: files[10].clone(),
                                 blocks_file: files[11].clone(),
                                 sblocks_file: files[12].clone(),
                             },
                             nums_file: files[13].clone()
                         },
                         pos_sp_o_adjacency_list_files: AdjacencyListFiles {
                             bitindex_files: BitIndexFiles {
                                 bits_file: files[14].clone(),
                                 blocks_file: files[15].clone(),
                                 sblocks_file: files[16].clone(),
                             },
                             nums_file: files[17].clone()
                         },
                         pos_o_ps_adjacency_list_files: AdjacencyListFiles {
                             bitindex_files: BitIndexFiles {
                                 bits_file: files[18].clone(),
                                 blocks_file: files[19].clone(),
                                 sblocks_file: files[20].clone(),
                             },
                             nums_file: files[21].clone()
                         },
                         neg_s_p_adjacency_list_files: AdjacencyListFiles {
                             bitindex_files: BitIndexFiles {
                                 bits_file: files[22].clone(),
                                 blocks_file: files[23].clone(),
                                 sblocks_file: files[24].clone(),
                             },
                             nums_file: files[25].clone()
                         },
                         neg_sp_o_adjacency_list_files: AdjacencyListFiles {
                             bitindex_files: BitIndexFiles {
                                 bits_file: files[26].clone(),
                                 blocks_file: files[27].clone(),
                                 sblocks_file: files[28].clone(),
                             },
                             nums_file: files[29].clone()
                         },
                         neg_o_ps_adjacency_list_files: AdjacencyListFiles {
                             bitindex_files: BitIndexFiles {
                                 bits_file: files[30].clone(),
                                 blocks_file: files[31].clone(),
                                 sblocks_file: files[32].clone(),
                             },
                             nums_file: files[33].clone()
                         },
                         pos_predicate_wavelet_tree_files: BitIndexFiles {
                             bits_file: files[34].clone(),
                             blocks_file: files[35].clone(),
                             sblocks_file: files[36].clone()
                         },
                         neg_predicate_wavelet_tree_files: BitIndexFiles {
                             bits_file: files[37].clone(),
                             blocks_file: files[38].clone(),
                             sblocks_file: files[39].clone()
                         },
                     };

                     layers.write()
                         .then(move |layers| {
                             layers.expect("rwlock write should always succeed").insert(name, (Some(parent), LayerFiles::Child(clf.clone())));
                             Ok(Box::new(SimpleLayerBuilder::from_parent(name, parent_layer, clf)) as Box<dyn LayerBuilder>)
                         })
                 }))
    }
}

#[derive(Clone)]
pub struct MemoryLabelStore {
    labels: futures_locks::RwLock<HashMap<String, Label>>
}

impl MemoryLabelStore {
    pub fn new() -> MemoryLabelStore {
        MemoryLabelStore {
            labels: futures_locks::RwLock::new(HashMap::new())
        }
    }
}

impl LabelStore for MemoryLabelStore {
    fn labels(&self) -> Box<dyn Future<Item=Vec<Label>,Error=std::io::Error>+Send> {
        Box::new(self.labels.read()
                 .then(|l| Ok(l.expect("rwlock read should always succeed")
                              .values().map(|v|v.clone()).collect())))
    }

    fn create_label(&self, name: &str) -> Box<dyn Future<Item=Label, Error=std::io::Error>+Send> {
        let label = Label::new_empty(name);

        Box::new(self.labels.write()
                 .then(move |l| {
                     let mut labels = l.expect("rwlock write should always succeed");
                     if labels.get(&label.name).is_some() {
                         Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "label already exists"))
                     }
                     else {
                         labels.insert(label.name.clone(), label.clone());
                         Ok(label)
                     }
                 }))
    }

    fn get_label(&self, name: &str) -> Box<dyn Future<Item=Option<Label>,Error=std::io::Error>+Send> {
        let name = name.to_owned();
        Box::new(self.labels.read()
                 .then(move |l| Ok(l.expect("rwlock read should always succeed")
                                   .get(&name).map(|label|label.clone()))))
    }

    fn set_label_option(&self, label: &Label, layer: Option<[u32;5]>) -> Box<dyn Future<Item=Option<Label>, Error=std::io::Error>+Send> {
        let new_label = label.with_updated_layer(layer);

        Box::new(self.labels.write()
                 .then(move |l| {
                     let mut labels = l.expect("rwlock write should always succeed");

                     match labels.get(&new_label.name) {
                         None => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "label does not exist")),
                         Some(old_label) => {
                             if old_label.version+1 != new_label.version {
                                 Ok(None)
                             }
                             else {
                                 labels.insert(new_label.name.clone(), new_label.clone());

                                 Ok(Some(new_label))
                             }
                         }
                     }
                 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::*;

    #[test]
    fn write_and_read_memory_backed() {
        let file = MemoryBackedStore::new();

        let w = file.open_write();
        let buf = tokio::io::write_all(w,[1,2,3])
            .and_then(move |_| tokio::io::read_to_end(file.open_read(), Vec::new()))
            .map(|(_,buf)| buf)
            .wait()
            .unwrap();

        assert_eq!(vec![1,2,3], buf);
    }

    #[test]
    fn write_and_map_memory_backed() {
        let file = MemoryBackedStore::new();

        let w = file.open_write();
        tokio::io::write_all(w,[1,2,3])
            .wait()
            .unwrap();

        assert_eq!(vec![1,2,3], *file.map().wait().unwrap().0);
    }

    #[test]
    fn create_layers_from_memory_store() {
        let store = MemoryLayerStore::new();
        let mut builder = store.create_base_layer().wait().unwrap();
        let base_name = builder.name();

        builder.add_string_triple(&StringTriple::new_value("cow","says","moo"));
        builder.add_string_triple(&StringTriple::new_value("pig","says","oink"));
        builder.add_string_triple(&StringTriple::new_value("duck","says","quack"));

        builder.commit_boxed().wait().unwrap();

        builder = store.create_child_layer(base_name).wait().unwrap();
        let child_name = builder.name();

        builder.remove_string_triple(&StringTriple::new_value("duck","says","quack"));
        builder.add_string_triple(&StringTriple::new_node("cow","likes","pig"));

        builder.commit_boxed().wait().unwrap();

        let layer = store.get_layer(child_name).wait().unwrap().unwrap();

        assert!(layer.string_triple_exists(&StringTriple::new_value("cow", "says", "moo")));
        assert!(layer.string_triple_exists(&StringTriple::new_value("pig", "says", "oink")));
        assert!(layer.string_triple_exists(&StringTriple::new_node("cow", "likes", "pig")));
        assert!(!layer.string_triple_exists(&StringTriple::new_value("duck", "says", "quack")));
    }

    #[test]
    fn memory_create_and_retrieve_equal_label() {
        let store = MemoryLabelStore::new();
        let foo = store.create_label("foo").wait().unwrap();
        assert_eq!(foo, store.get_label("foo").wait().unwrap().unwrap());
    }

    #[test]
    fn memory_update_label_succeeds() {
        let store = MemoryLabelStore::new();
        let foo = store.create_label("foo").wait().unwrap();

        assert_eq!(1, store.set_label(&foo, [6,7,8,9,10]).wait().unwrap().unwrap().version);

        assert_eq!(1, store.get_label("foo").wait().unwrap().unwrap().version);
    }

    #[test]
    fn memory_update_label_twice_from_same_label_object_fails() {
        let store = MemoryLabelStore::new();
        let foo = store.create_label("foo").wait().unwrap();

        assert!(store.set_label(&foo, [6,7,8,9,10]).wait().unwrap().is_some());
        assert!(store.set_label(&foo, [1,1,1,1,1]).wait().unwrap().is_none());
    }

    #[test]
    fn memory_update_label_twice_from_updated_label_object_succeeds() {
        let store = MemoryLabelStore::new();
        let foo = store.create_label("foo").wait().unwrap();

        let foo2 = store.set_label(&foo, [6,7,8,9,10]).wait().unwrap().unwrap();
        assert!(store.set_label(&foo2, [1,1,1,1,1]).wait().unwrap().is_some());
    }

}
