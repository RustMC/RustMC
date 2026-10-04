//! Anvil region-file reader (`.mca`) for owner-provided world saves.
//! Format: publicly documented at <https://minecraft.wiki/w/Region_file_format>.
//! Only chunk streams the user's own client produced are ever read; no Mojang
//! data ships with RustMC.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;

use crate::nbt;

pub struct Region {
    coords: (i32, i32),
    bytes: Vec<u8>,
    path: PathBuf,
}

impl Region {
    fn open(path: PathBuf, coords: (i32, i32)) -> Result<Self, String> {
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if bytes.len() < 8192 {
            return Err(format!(
                "{} is smaller than a region header",
                path.display()
            ));
        }
        Ok(Self {
            coords,
            bytes,
            path,
        })
    }

    fn be_i32(&self, byte_offset: usize) -> i32 {
        let b = &self.bytes[byte_offset..byte_offset + 4];
        i32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }

    /// Decompressed chunk NBT bytes, or None when the chunk is not stored.
    fn chunk_bytes(&self, local_x: usize, local_z: usize) -> Result<Option<Vec<u8>>, String> {
        let index = local_x + local_z * 32;
        let location = self.be_i32(index * 4);
        if location == 0 {
            return Ok(None);
        }
        let sector = ((location as u32) >> 8) as usize;
        let sectors = (location as u32 & 0xff) as usize;
        let offset = sector.checked_mul(4096).ok_or("region offset overflow")?;
        if sectors == 0 {
            return Err(format!(
                "chunk {index} in {} has no sectors",
                self.path.display()
            ));
        }
        if offset + 8 > self.bytes.len() {
            return Err(format!(
                "chunk {index} in {} points outside the file",
                self.path.display()
            ));
        }
        let b = &self.bytes[offset..offset + 4];
        // The stored length includes the compression byte, and need not fill
        // the final allocated sector. 26.3 saves may end at the final payload
        // byte without padding to a 4 KiB boundary.
        let len = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
        if len == 0 || len + 4 > sectors * 4096 {
            return Err(format!(
                "chunk {index} in {} has invalid length",
                self.path.display()
            ));
        }
        let compression = self.bytes[offset + 4];
        let end = offset
            .checked_add(4)
            .and_then(|x| x.checked_add(len))
            .ok_or("chunk length overflow")?;
        if end > self.bytes.len() {
            return Err(format!(
                "chunk {index} in {} extends past the file",
                self.path.display()
            ));
        }
        let data = &self.bytes[offset + 5..end];
        Ok(Some(match compression {
            2 => {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut ZlibDecoder::new(data), &mut out)
                    .map_err(|e| format!("zlib failure in region {:?}: {e}", self.coords))?;
                out
            }
            3 => data.to_vec(),
            other => {
                return Err(format!(
                    "unsupported chunk compression type {other} in {:?}",
                    self.coords
                ));
            }
        }))
    }
}

/// Lazily opened cache of region files for one world directory.
pub struct RegionStore {
    world_dir: PathBuf,
    regions: BTreeMap<(i32, i32), Region>,
    layout: Option<Layout>,
}

#[derive(Clone, Copy)]
enum Layout {
    /// Pre-26.3 single-player layout.
    Root,
    /// 26.3 layout: `dimensions/minecraft/overworld/region`.
    Dimensions,
}

impl Layout {
    fn dir(self) -> &'static Path {
        match self {
            Self::Root => Path::new("region"),
            Self::Dimensions => Path::new("dimensions/minecraft/overworld/region"),
        }
    }
}

impl RegionStore {
    pub fn new(world_dir: &Path) -> Self {
        Self {
            world_dir: world_dir.to_path_buf(),
            regions: BTreeMap::new(),
            layout: None,
        }
    }

    fn region_path(&self, coords: (i32, i32), layout: Layout) -> PathBuf {
        let name = format!("r.{}.{}.mca", coords.0, coords.1);
        self.world_dir.join(layout.dir()).join(name)
    }

    pub fn chunk_root(&mut self, chunk_x: i32, chunk_z: i32) -> Result<Option<nbt::Tag>, String> {
        match self.chunk_raw(chunk_x, chunk_z)? {
            None => Ok(None),
            Some(bytes) => Ok(Some(nbt::parse_root(&bytes)?)),
        }
    }

    /// Decompressed chunk NBT bytes, or None when not stored/generated.
    pub fn chunk_raw(&mut self, chunk_x: i32, chunk_z: i32) -> Result<Option<Vec<u8>>, String> {
        let coords = (chunk_x.div_euclid(32), chunk_z.div_euclid(32));
        if !self.regions.contains_key(&coords) {
            let paths = match self.layout {
                None => vec![Layout::Root, Layout::Dimensions],
                Some(Layout::Root) => vec![Layout::Root],
                Some(Layout::Dimensions) => vec![Layout::Dimensions],
            };
            let mut opened = None;
            for layout in paths {
                let path = self.region_path(coords, layout);
                if let Ok(region) = Region::open(path, coords) {
                    self.layout = Some(layout);
                    opened = Some(region);
                    break;
                }
            }
            match opened {
                Some(region) => {
                    self.regions.insert(coords, region);
                }
                None => return Ok(None),
            }
        }
        let region = &self.regions[&coords];
        let local_x = chunk_x.rem_euclid(32) as usize;
        let local_z = chunk_z.rem_euclid(32) as usize;
        region.chunk_bytes(local_x, local_z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{Tag, compound, write_root};
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    /// One region file with a single stored chunk at local (3, 5), sector 2.
    fn synthetic_region(chunk_nbt: &Tag) -> Vec<u8> {
        let mut root = Vec::new();
        write_root(chunk_nbt, &mut root);
        let compressed = zlib(&root);
        let mut file = vec![0u8; 8192 + 4096];
        let index = 3 + 5 * 32;
        let location: i32 = (2 << 8) | 1; // sector 2, one sector long
        file[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());
        let payload_start = 8192; // sector 2: first sector after the 8 KiB header
        file[payload_start..payload_start + 4]
            .copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        file[payload_start + 4] = 2;
        file[payload_start + 5..payload_start + 5 + compressed.len()].copy_from_slice(&compressed);
        file
    }

    fn store_with_region(dir: &Path, region: Vec<u8>) {
        std::fs::create_dir_all(dir.join("region")).unwrap();
        std::fs::write(dir.join("region").join("r.0.0.mca"), region).unwrap();
    }

    #[test]
    fn reads_a_zlib_chunk_and_reports_absent_chunks() {
        let dir = std::env::temp_dir().join("rustmc-oracle-read-test");
        let _ = std::fs::remove_dir_all(&dir);
        let chunk = compound(&[
            ("xPos", Tag::Int(3)),
            ("zPos", Tag::Int(5)),
            ("hello", Tag::String("world".into())),
        ]);
        store_with_region(&dir, synthetic_region(&chunk));

        let mut store = RegionStore::new(&dir);
        let root = store
            .chunk_root(3, 5)
            .unwrap()
            .expect("chunk should be present");
        assert_eq!(root.get("hello").and_then(Tag::as_str), Some("world"));
        assert_eq!(root.get("xPos").and_then(Tag::as_i32), Some(3));
        // Same chunk via the other local slot of the cache: absent chunk.
        assert!(store.chunk_root(4, 5).unwrap().is_none());
        // Missing region file is simply "no data", not an error.
        assert!(store.chunk_root(800, 800).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepts_a_chunk_whose_payload_ends_at_file_eof() {
        let dir =
            std::env::temp_dir().join(format!("rustmc-oracle-exact-eof-{}", std::process::id()));
        let chunk = compound(&[("xPos", Tag::Int(3)), ("zPos", Tag::Int(5))]);
        let mut region = synthetic_region(&chunk);
        let start = 8192;
        let len = u32::from_be_bytes(region[start..start + 4].try_into().unwrap()) as usize;
        region.truncate(start + 4 + len);
        store_with_region(&dir, region);
        let mut store = RegionStore::new(&dir);
        assert!(store.chunk_root(3, 5).unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
