use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

pub struct Vfd {
    file: File,
}

impl Vfd {
    pub fn create_new(path: &Path) -> io::Result<Self> {
        let file = File::create_new(path)?;
        Ok(Self { file })
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::options().read(true).write(true).open(path)?;
        Ok(Self { file })
    }

    pub fn size(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.file.read_exact_at(buf, offset)
    }

    pub fn write_all_at(&self, buf: &[u8], offset: u64) -> io::Result<()> {
        self.file.write_all_at(buf, offset)
    }

    pub fn sync_data(&self) -> io::Result<()> {
        self.file.sync_data()
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use tempfile::{TempDir, tempdir};

    use super::*;

    fn new_file() -> io::Result<(TempDir, Vfd)> {
        let dir = tempdir()?;
        let file = Vfd::create_new(&dir.path().join("data"))?;
        Ok((dir, file))
    }

    /* AI GENERATED TEST */
    #[test]
    fn create_new_fails_if_file_exists() -> io::Result<()> {
        let (dir, _file) = new_file()?;
        let again = Vfd::create_new(&dir.path().join("data"));
        assert_eq!(
            again.err().map(|error| error.kind()),
            Some(ErrorKind::AlreadyExists)
        );
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn open_missing_file_fails() -> io::Result<()> {
        let dir = tempdir()?;
        let opened = Vfd::open(&dir.path().join("data"));
        assert_eq!(
            opened.err().map(|error| error.kind()),
            Some(ErrorKind::NotFound)
        );
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn size_follows_writes() -> io::Result<()> {
        let (_dir, file) = new_file()?;
        assert_eq!(file.size()?, 0);
        file.write_all_at(&[1; 100], 0)?;
        assert_eq!(file.size()?, 100);
        file.write_all_at(&[1; 10], 500)?;
        assert_eq!(file.size()?, 510);
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn write_then_read_at_offsets() -> io::Result<()> {
        let (_dir, file) = new_file()?;
        file.write_all_at(&[1; 300], 0)?;
        file.write_all_at(&[2; 300], 100)?;

        let mut buf = [0; 300];
        file.read_exact_at(&mut buf, 0)?;
        assert!(buf[..100].iter().all(|&byte| byte == 1));
        assert!(buf[100..].iter().all(|&byte| byte == 2));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn read_past_end_is_unexpected_eof() -> io::Result<()> {
        let (_dir, file) = new_file()?;
        file.write_all_at(&[1; 100], 0)?;

        let mut buf = [0; 200];
        let error = file
            .read_exact_at(&mut buf, 0)
            .expect_err("only 100 bytes exist");
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn interleaved_offsets_ignore_the_cursor() -> io::Result<()> {
        let (_dir, file) = new_file()?;
        file.write_all_at(&[3; 50], 50)?;
        file.write_all_at(&[4; 50], 0)?;

        let mut first = [0; 50];
        let mut second = [0; 50];
        file.read_exact_at(&mut second, 50)?;
        file.read_exact_at(&mut first, 0)?;
        assert_eq!(first, [4; 50]);
        assert_eq!(second, [3; 50]);
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn reopen_sees_synced_data() -> io::Result<()> {
        let (dir, file) = new_file()?;
        file.write_all_at(&[7; 64], 0)?;
        file.sync_data()?;
        drop(file);

        let file = Vfd::open(&dir.path().join("data"))?;
        assert_eq!(file.size()?, 64);
        let mut buf = [0; 64];
        file.read_exact_at(&mut buf, 0)?;
        assert_eq!(buf, [7; 64]);
        file.write_all_at(&[8; 1], 0)?;
        Ok(())
    }
}
