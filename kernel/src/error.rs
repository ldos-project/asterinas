// SPDX-License-Identifier: MPL-2.0

/// Error number.
///
/// This should match the Linux error numbers as defined in `errno.h` and similar.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Errno {
    /// Operation not permitted
    EPERM = 1,
    /// No such file or directory
    ENOENT = 2,
    /// No such process
    ESRCH = 3,
    /// Interrupted system call
    EINTR = 4,
    /// I/O error
    EIO = 5,
    /// No such device or address
    ENXIO = 6,
    /// Argument list too long
    E2BIG = 7,
    /// Exec format error
    ENOEXEC = 8,
    /// Bad file number
    EBADF = 9,
    /// No child processes
    ECHILD = 10,
    /// Try again
    EAGAIN = 11,
    /// Out of memory
    ENOMEM = 12,
    /// Permission denied
    EACCES = 13,
    /// Bad address
    EFAULT = 14,
    /// Block device required
    ENOTBLK = 15,
    /// Device or resource busy
    EBUSY = 16,
    /// File exists
    EEXIST = 17,
    /// Cross-device link
    EXDEV = 18,
    /// No such device
    ENODEV = 19,
    /// Not a directory
    ENOTDIR = 20,
    /// Is a directory
    EISDIR = 21,
    /// Invalid argument
    EINVAL = 22,
    /// File table overflow
    ENFILE = 23,
    /// Too many open files
    EMFILE = 24,
    /// Not a typewriter
    ENOTTY = 25,
    /// Text file busy
    ETXTBSY = 26,
    /// File too large
    EFBIG = 27,
    /// No space left on device
    ENOSPC = 28,
    /// Illegal seek
    ESPIPE = 29,
    /// Read-only file system
    EROFS = 30,
    /// Too many links
    EMLINK = 31,
    /// Broken pipe
    EPIPE = 32,
    /// Math argument out of domain of func
    EDOM = 33,
    /// Math result not representable
    ERANGE = 34,

    /// Resource deadlock would occur
    EDEADLK = 35,
    /// File name too long
    ENAMETOOLONG = 36,
    /// No record locks available
    ENOLCK = 37,
    /*
     * This error code is special: arch syscall entry code will return
     * -ENOSYS if users try to call a syscall that doesn't exist.  To keep
     * failures of syscalls that really do exist distinguishable from
     * failures due to attempts to use a nonexistent syscall, syscall
     * implementations should refrain from returning -ENOSYS.
     */
    /// Invalid system call number
    ENOSYS = 38,
    /// Directory not empty
    ENOTEMPTY = 39,
    /// Too many symbolic links encountered
    ELOOP = 40,
    // EWOULDBLOCK	EAGAIN	/* Operation would block */
    /// No message of desired type
    ENOMSG = 42,
    /// Identifier removed
    EIDRM = 43,
    /// Channel number out of range
    ECHRNG = 44,
    /// Level 2 not synchronized
    EL2NSYNC = 45,
    /// Level 3 halted
    EL3HLT = 46,
    /// Level 3 reset
    EL3RST = 47,
    /// Link number out of range
    ELNRNG = 48,
    /// Protocol driver not attached
    EUNATCH = 49,
    /// No CSI structure available
    ENOCSI = 50,
    /// Level 2 halted
    EL2HLT = 51,
    /// Invalid exchange
    EBADE = 52,
    /// Invalid request descriptor
    EBADR = 53,
    /// Exchange full
    EXFULL = 54,
    /// No anode
    ENOANO = 55,
    /// Invalid request code
    EBADRQC = 56,
    /// Invalid slot
    EBADSLT = 57,
    // EDEADLOCK	EDEADLK
    /// Bad font file format
    EBFONT = 59,
    /// Device not a stream
    ENOSTR = 60,
    /// No data available
    ENODATA = 61,
    /// Timer expired
    ETIME = 62,
    /// Out of streams resources
    ENOSR = 63,
    /// Machine is not on the network
    ENONET = 64,
    /// Package not installed
    ENOPKG = 65,
    /// Object is remote
    EREMOTE = 66,
    /// Link has been severed
    ENOLINK = 67,
    /// Advertise error
    EADV = 68,
    /// Srmount error
    ESRMNT = 69,
    /// Communication error on send
    ECOMM = 70,
    /// Protocol error
    EPROTO = 71,
    /// Multihop attempted
    EMULTIHOP = 72,
    /// RFS specific error
    EDOTDOT = 73,
    /// Not a data message
    EBADMSG = 74,
    /// Value too large for defined data type
    EOVERFLOW = 75,
    /// Name not unique on network
    ENOTUNIQ = 76,
    /// File descriptor in bad state
    EBADFD = 77,
    /// Remote address changed
    EREMCHG = 78,
    /// Can not access a needed shared library
    ELIBACC = 79,
    /// Accessing a corrupted shared library
    ELIBBAD = 80,
    /// .lib section in a.out corrupted
    ELIBSCN = 81,
    /// Attempting to link in too many shared libraries
    ELIBMAX = 82,
    /// Cannot exec a shared library directly
    ELIBEXEC = 83,
    /// Illegal byte sequence
    EILSEQ = 84,
    /// Interrupted system call should be restarted
    ERESTART = 85,
    /// Streams pipe error
    ESTRPIPE = 86,
    /// Too many users
    EUSERS = 87,
    /// Socket operation on non-socket
    ENOTSOCK = 88,
    /// Destination address required
    EDESTADDRREQ = 89,
    /// Message too long
    EMSGSIZE = 90,
    /// Protocol wrong type for socket
    EPROTOTYPE = 91,
    /// Protocol not available
    ENOPROTOOPT = 92,
    /// Protocol not supported
    EPROTONOSUPPORT = 93,
    /// Socket type not supported
    ESOCKTNOSUPPORT = 94,
    /// Operation not supported on transport endpoint
    EOPNOTSUPP = 95,
    /// Protocol family not supported
    EPFNOSUPPORT = 96,
    /// Address family not supported by protocol
    EAFNOSUPPORT = 97,
    /// Address already in use
    EADDRINUSE = 98,
    /// Cannot assign requested address
    EADDRNOTAVAIL = 99,
    /// Network is down
    ENETDOWN = 100,
    /// Network is unreachable
    ENETUNREACH = 101,
    /// Network dropped connection because of reset
    ENETRESET = 102,
    /// Software caused connection abort
    ECONNABORTED = 103,
    /// Connection reset by peer
    ECONNRESET = 104,
    /// No buffer space available
    ENOBUFS = 105,
    /// Transport endpoint is already connected
    EISCONN = 106,
    /// Transport endpoint is not connected
    ENOTCONN = 107,
    /// Cannot send after transport endpoint shutdown
    ESHUTDOWN = 108,
    /// Too many references: cannot splice
    ETOOMANYREFS = 109,
    /// Connection timed out
    ETIMEDOUT = 110,
    /// Connection refused
    ECONNREFUSED = 111,
    /// Host is down
    EHOSTDOWN = 112,
    /// No route to host
    EHOSTUNREACH = 113,
    /// Operation already in progress
    EALREADY = 114,
    /// Operation now in progress
    EINPROGRESS = 115,
    /// Stale file handle
    ESTALE = 116,
    /// Structure needs cleaning
    EUCLEAN = 117,
    /// Not a XENIX named type file
    ENOTNAM = 118,
    /// No XENIX semaphores available
    ENAVAIL = 119,
    /// Is a named type file
    EISNAM = 120,
    /// Remote I/O error
    EREMOTEIO = 121,
    /// Quota exceeded
    EDQUOT = 122,
    /// No medium found
    ENOMEDIUM = 123,
    /// Wrong medium type
    EMEDIUMTYPE = 124,
    /// Operation Canceled
    ECANCELED = 125,
    /// Required key not available
    ENOKEY = 126,
    /// Key has expired
    EKEYEXPIRED = 127,
    /// Key has been revoked
    EKEYREVOKED = 128,
    /// Key was rejected by service
    EKEYREJECTED = 129,
    /// Owner died (for robust mutexes)
    EOWNERDEAD = 130,
    /// State not recoverable
    ENOTRECOVERABLE = 131,

    /// Operation not possible due to RF-kill
    ERFKILL = 132,

    /// Memory page has hardware error
    EHWPOISON = 133,

    /// Restart of an interrupted system call. For kernel internal use only.
    ERESTARTSYS = 512,
}

/// Error used in this crate.
///
/// This wraps an [`Errno`] and potentally provides additional information. The error number is returned to the
/// userspace from the syscall if the error reaches syscall invocation.
///
/// This type is convertable to and from various other error types.
#[derive(Debug, Clone, Copy)]
pub struct Error {
    errno: Errno,
    msg: Option<&'static str>,
}

impl Error {
    pub const fn new(errno: Errno) -> Self {
        Error { errno, msg: None }
    }

    pub const fn with_message(errno: Errno, msg: &'static str) -> Self {
        Error {
            errno,
            msg: Some(msg),
        }
    }

    pub const fn error(&self) -> Errno {
        self.errno
    }
}

impl core::error::Error for Error {}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.errno)?;
        if let Some(msg) = self.msg {
            f.write_str(msg)?;
        }
        Ok(())
    }
}

impl From<Errno> for Error {
    fn from(errno: Errno) -> Self {
        Error::new(errno)
    }
}

impl AsRef<Error> for Error {
    fn as_ref(&self) -> &Error {
        self
    }
}

impl From<ostd::tables::TableAttachError> for Error {
    fn from(value: ostd::tables::TableAttachError) -> Self {
        match value {
            ostd::tables::TableAttachError::Unsupported { .. } => {
                Error::with_message(Errno::EPERM, "table feature unsupported")
            }
            ostd::tables::TableAttachError::AllocationFailed { .. } => {
                Error::with_message(Errno::EBUSY, "table cannot be attached to")
            }
            ostd::tables::TableAttachError::Whatever { .. } => {
                Error::with_message(Errno::EINVAL, "unknown error")
            }
        }
    }
}

impl From<ostd::Error> for Error {
    fn from(ostd_error: ostd::Error) -> Self {
        match ostd_error {
            ostd::Error::AccessDenied => Error::new(Errno::EFAULT),
            ostd::Error::NoMemory => Error::new(Errno::ENOMEM),
            ostd::Error::InvalidArgs => Error::new(Errno::EINVAL),
            ostd::Error::IoError => Error::new(Errno::EIO),
            ostd::Error::NotEnoughResources => Error::new(Errno::EBUSY),
            ostd::Error::PageFault => Error::new(Errno::EFAULT),
            ostd::Error::Overflow => Error::new(Errno::EOVERFLOW),
            ostd::Error::MapAlreadyMappedVaddr => Error::new(Errno::EINVAL),
            ostd::Error::KVirtAreaAllocError => Error::new(Errno::ENOMEM),
        }
    }
}

impl From<(ostd::Error, usize)> for Error {
    // Used in fallible memory read/write API
    fn from(ostd_error: (ostd::Error, usize)) -> Self {
        Error::from(ostd_error.0)
    }
}

impl From<aster_block::bio::BioEnqueueError> for Error {
    fn from(error: aster_block::bio::BioEnqueueError) -> Self {
        match error {
            aster_block::bio::BioEnqueueError::IsFull => {
                Error::with_message(Errno::EBUSY, "The request queue is full")
            }
            aster_block::bio::BioEnqueueError::Refused => {
                Error::with_message(Errno::EBUSY, "Refuse to enqueue the bio")
            }
            aster_block::bio::BioEnqueueError::TooBig => {
                Error::with_message(Errno::EINVAL, "Bio is too big")
            }
        }
    }
}

impl From<aster_block::bio::BioStatus> for Error {
    fn from(err_status: aster_block::bio::BioStatus) -> Self {
        match err_status {
            aster_block::bio::BioStatus::NotSupported => {
                Error::with_message(Errno::EIO, "I/O operation is not supported")
            }
            aster_block::bio::BioStatus::NoSpace => {
                Error::with_message(Errno::ENOSPC, "Insufficient space on device")
            }
            aster_block::bio::BioStatus::IoError => {
                Error::with_message(Errno::EIO, "I/O operation fails")
            }
            status => panic!("Can not convert the status: {:?} to an error", status),
        }
    }
}

impl From<core::num::TryFromIntError> for Error {
    fn from(_: core::num::TryFromIntError) -> Self {
        Error::with_message(Errno::EINVAL, "Invalid integer")
    }
}

impl From<core::str::Utf8Error> for Error {
    fn from(_: core::str::Utf8Error) -> Self {
        Error::with_message(Errno::EINVAL, "Invalid utf-8 string")
    }
}

impl From<alloc::string::FromUtf8Error> for Error {
    fn from(_: alloc::string::FromUtf8Error) -> Self {
        Error::with_message(Errno::EINVAL, "Invalid utf-8 string")
    }
}

impl From<core::ffi::FromBytesUntilNulError> for Error {
    fn from(_: core::ffi::FromBytesUntilNulError) -> Self {
        Error::with_message(Errno::E2BIG, "Cannot find null in cstring")
    }
}

impl From<core::ffi::FromBytesWithNulError> for Error {
    fn from(_: core::ffi::FromBytesWithNulError) -> Self {
        Error::with_message(Errno::E2BIG, "Cannot find null in cstring")
    }
}

impl From<cpio_decoder::error::Error> for Error {
    fn from(cpio_error: cpio_decoder::error::Error) -> Self {
        match cpio_error {
            cpio_decoder::error::Error::MagicError => {
                Error::with_message(Errno::EINVAL, "CPIO invalid magic number")
            }
            cpio_decoder::error::Error::Utf8Error => {
                Error::with_message(Errno::EINVAL, "CPIO invalid utf-8 string")
            }
            cpio_decoder::error::Error::ParseIntError => {
                Error::with_message(Errno::EINVAL, "CPIO parse int error")
            }
            cpio_decoder::error::Error::FileTypeError => {
                Error::with_message(Errno::EINVAL, "CPIO invalid file type")
            }
            cpio_decoder::error::Error::FileNameError => {
                Error::with_message(Errno::EINVAL, "CPIO invalid file name")
            }
            cpio_decoder::error::Error::BufferShortError => {
                Error::with_message(Errno::EINVAL, "CPIO buffer is too short")
            }
            cpio_decoder::error::Error::IoError => {
                Error::with_message(Errno::EIO, "CPIO buffer I/O error")
            }
        }
    }
}

impl From<Error> for ostd::Error {
    fn from(error: Error) -> Self {
        match error.errno {
            Errno::EACCES => ostd::Error::AccessDenied,
            Errno::EIO => ostd::Error::IoError,
            Errno::ENOMEM => ostd::Error::NoMemory,
            Errno::EFAULT => ostd::Error::PageFault,
            Errno::EINVAL => ostd::Error::InvalidArgs,
            Errno::EBUSY => ostd::Error::NotEnoughResources,
            _ => ostd::Error::InvalidArgs,
        }
    }
}

impl From<alloc::ffi::NulError> for Error {
    fn from(_: alloc::ffi::NulError) -> Self {
        Error::with_message(Errno::E2BIG, "Cannot find null in cstring")
    }
}

impl From<int_to_c_enum::TryFromIntError> for Error {
    fn from(_: int_to_c_enum::TryFromIntError) -> Self {
        Error::with_message(Errno::EINVAL, "Invalid enum value")
    }
}

impl From<aster_systree::Error> for Error {
    fn from(err: aster_systree::Error) -> Self {
        use aster_systree::Error::*;
        match err {
            NodeNotFound(_) => Error::new(Errno::ENOENT),
            InvalidNodeOperation(_) => Error::new(Errno::EINVAL),
            AttributeError => Error::new(Errno::EIO),
            PermissionDenied => Error::new(Errno::EACCES),
            InternalError(msg) => Error::with_message(Errno::EIO, msg),
            Overflow => Error::new(Errno::EOVERFLOW),
        }
    }
}

#[macro_export]
macro_rules! return_errno {
    ($errno: expr) => {
        return Err($crate::error::Error::new($errno))
    };
}

#[macro_export]
macro_rules! return_errno_with_message {
    ($errno: expr, $message: expr) => {
        return Err($crate::error::Error::with_message($errno, $message))
    };
}
