//! Ipld dag.
use crate::cid::Cid;
use crate::error::{BlockError, IpldError};
use crate::ipld::Ipld;
use crate::path::Path;
use crate::store::{Store, StoreIpldExt};
use failure::Fail;

/// Dag error.
#[derive(Debug, Fail)]
pub enum DagError {
    /// Path segment is not a number.
    #[fail(display = "Path segment is not a number.")]
    NotNumber(std::num::ParseIntError),
    /// Cannot index into ipld.
    #[fail(display = "Cannot index into")]
    NotIndexable,
    /// Ipld error.
    #[fail(display = "{}", _0)]
    Ipld(IpldError),
    /// Block error.
    #[fail(display = "{}", _0)]
    Block(BlockError),
}

impl From<std::num::ParseIntError> for DagError {
    fn from(err: std::num::ParseIntError) -> Self {
        Self::NotNumber(err)
    }
}

impl From<IpldError> for DagError {
    fn from(err: IpldError) -> Self {
        Self::Ipld(err)
    }
}

impl From<BlockError> for DagError {
    fn from(err: BlockError) -> Self {
        Self::Block(err)
    }
}

/// Path in a dag.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct DagPath<'a>(&'a Cid, Path);

impl<'a> DagPath<'a> {
    /// Create a new dag path.
    pub fn new<T: Into<Path>>(cid: &'a Cid, path: T) -> Self {
        Self(cid, path.into())
    }
}

impl<'a> From<&'a Cid> for DagPath<'a> {
    fn from(cid: &'a Cid) -> Self {
        Self(cid, Default::default())
    }
}

/// Extends a store with path querying.
pub trait StoreDagExt {
    /// Retrives ipld from the dag.
    fn get<'a>(&self, path: &DagPath<'a>) -> Result<Option<Ipld>, DagError>;
}

impl<TStore: Store> StoreDagExt for TStore {
    fn get<'a>(&self, path: &DagPath<'a>) -> Result<Option<Ipld>, DagError> {
        let root = self.read_ipld(&path.0)?;
        let mut root = if let Some(root) = root {
            root
        } else {
            return Ok(None);
        };
        let mut ipld = &root;
        for segment in path.1.iter() {
            if let Some(next) = match ipld {
                Ipld::List(_) => {
                    let index: usize = segment.parse()?;
                    ipld.get(index)
                }
                Ipld::Map(_) => ipld.get(segment.as_str()),
                _ => return Err(DagError::NotIndexable),
            } {
                if let Ipld::Link(cid) = next {
                    if let Some(cid) = self.read_ipld(cid)? {
                        root = cid;
                        ipld = &root;
                    } else {
                        return Ok(None);
                    }
                } else {
                    ipld = next;
                }
            } else {
                return Ok(None);
            }
        }
        Ok(Some(ipld.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipld;
    use crate::store::{BufStore, MemStore, StoreCborExt};
    use crate::DefaultHash as H;

    #[test]
    fn test_dag() {
        let store = BufStore::new(MemStore::default(), 16, 16);
        let ipld1 = ipld!({"a": 3});
        let cid = store.write_cbor::<H, _>(&ipld1).unwrap();
        let ipld2 = ipld!({"root": [{"child": &cid}]});
        let root = store.write_cbor::<H, _>(&ipld2).unwrap();
        let path = DagPath::new(&root, "root/0/child/a");
        assert_eq!(store.get(&path).unwrap(), Some(Ipld::Integer(3)));
    }
}
