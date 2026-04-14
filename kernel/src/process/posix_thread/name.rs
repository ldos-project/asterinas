// SPDX-License-Identifier: MPL-2.0

use core::fmt::{Display, Formatter};

use crate::prelude::*;

pub const MAX_THREAD_NAME_LEN: usize = 16;

#[derive(Debug, Clone)]
pub struct ThreadName {
    inner: [u8; MAX_THREAD_NAME_LEN],
    count: usize,
}

impl Display for ThreadName {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.write_str(
            &self
                .name()
                .unwrap_or_default()
                .map(|v| v.to_string_lossy())
                .unwrap_or("[unnamed]".into()),
        )
    }
}

impl Default for ThreadName {
    fn default() -> Self {
        ThreadName::new()
    }
}

impl ThreadName {
    pub fn new() -> Self {
        ThreadName {
            inner: [0; MAX_THREAD_NAME_LEN],
            count: 0,
        }
    }

    pub fn new_from_executable_path(executable_path: &str) -> Result<Self> {
        let mut thread_name = ThreadName::new();
        let executable_file_name = executable_path
            .split('/')
            .next_back()
            .ok_or(Error::with_message(Errno::EINVAL, "invalid elf path"))?;
        let name = CString::new(executable_file_name)?;
        thread_name.set_name(&name)?;
        Ok(thread_name)
    }

    pub fn set_name(&mut self, name: &CStr) -> Result<()> {
        let bytes = name.to_bytes_with_nul();
        let bytes_len = bytes.len();
        if bytes_len > MAX_THREAD_NAME_LEN {
            // if len > MAX_THREAD_NAME_LEN, truncate it.
            self.count = MAX_THREAD_NAME_LEN;
            self.inner[..MAX_THREAD_NAME_LEN].clone_from_slice(&bytes[..MAX_THREAD_NAME_LEN]);
            self.inner[MAX_THREAD_NAME_LEN - 1] = 0;
            return Ok(());
        }
        self.count = bytes_len;
        self.inner[..bytes_len].clone_from_slice(bytes);
        Ok(())
    }

    pub fn name(&self) -> Result<Option<&CStr>> {
        Ok(Some(CStr::from_bytes_until_nul(&self.inner)?))
    }
}

#[cfg(ktest)]
mod test {
    use alloc::format;

    use ostd::prelude::*;

    use super::*;

    #[ktest]
    fn display_shows_name() {
        let mut name = ThreadName::new();
        name.set_name(c"hello").unwrap();
        assert_eq!(format!("{}", name), "hello");

        name.set_name(c"hel\xFFo").unwrap();
        assert_eq!(format!("{}", name), "hel\u{FFFD}o");
    }

    #[ktest]
    fn set_name_truncates_when_too_long() {
        // 16 chars + nul = 17 bytes > MAX_THREAD_NAME_LEN
        let mut name = ThreadName::new();
        let cname = CString::new("1234567890123456").unwrap();
        name.set_name(&cname).unwrap();
        assert_eq!(name.name().unwrap().unwrap().to_bytes(), b"123456789012345");
    }

    #[ktest]
    fn from_executable_path_extracts_filename() {
        let name = ThreadName::new_from_executable_path("/usr/bin/myapp").unwrap();
        assert_eq!(name.name().unwrap().unwrap().to_bytes(), b"myapp");

        let name = ThreadName::new_from_executable_path("myapp").unwrap();
        assert_eq!(name.name().unwrap().unwrap().to_bytes(), b"myapp");
    }
}
