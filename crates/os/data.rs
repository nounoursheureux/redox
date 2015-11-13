/// A data node (file/dir)
pub enum Data {
    /// File
    File(File),
    /// Directory
    Dir(Dir),
}

/// A file
pub struct File {
    /// The name of the file
    name: String,
    /// The actual content of the file
    data: Vec<u8>,
}

impl File {
    /// Create a file from a slice of bytes
    pub fn from_bytes(b: &[u8]) -> Self {
        let name = unsafe {
            String::from_utf8_unchecked(b[0..64].to_vec())
        };
        let data = b[257..].to_vec();

        File {
            name: name,
            data: data,
        }
    }
}

/// A directory
pub struct Dir {
    /// The name of the directory
    name: String,
    /// The table of the directory
    data: Vec<u8>,
}

impl Dir {
    /// Create a new directory from a slice of bytes
    pub fn from_bytes(b: &[u8]) -> Self {
        let name = unsafe {
            String::from_utf8_unchecked(b[0..64].to_vec())
        };
        let mut n = 0;
        while let Some(35) = b.get(n + 256 - 1) {
            n += 256;
        }

        let data = b[n..].to_vec();

        Dir {
            name: name,
            data: data,
        }
    }

    /// Get the table represented by this directory
    pub fn get_table(&'a self) -> NodeTable<'a> {
        NodeTable::new(&self.data[..])
    }
}
