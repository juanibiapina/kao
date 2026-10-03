use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub fn clone_path(source: &Path, destination: &Path) -> io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    if unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0x0001) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
