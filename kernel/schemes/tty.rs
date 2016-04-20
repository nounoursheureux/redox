use alloc::boxed::Box;
use fs::{KScheme, Resource, Url};
use system::error::{Error, Result, ENOENT};

pub struct TtyScheme;

impl KScheme for TtyScheme {
    fn scheme(&self) ->  &str {
        "tty"
    }

    fn open(&mut self, url: Url, _: usize) -> Result<Box<Resource>> {
        let name = url.reference();
        if let Ok(num) = name.parse::<usize>() {
            let console_manager = ::env().console_manager.lock();
            if console_manager.get(num).is_ok() {
                Ok(Box::new(TtyResource { index: num }))
            } else {
                Err(Error::new(ENOENT))
            }
        } else {
            Err(Error::new(ENOENT))
        }
    }
}

pub struct TtyResource {
    index: usize
}

impl Resource for TtyResource {
    fn dup(&self) -> Result<Box<Resource>> {
        Ok(Box::new(TtyResource { index: self.index }))
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut console_manager = ::env().console_manager.lock();
        let mut console = try!(console_manager.get_mut(self.index));
        console.write(buf);
        Ok(buf.len())
    }
}
