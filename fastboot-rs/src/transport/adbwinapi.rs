use std::{
    collections::HashMap,
    env,
    ffi::c_void,
    fmt, io,
    mem::{size_of, MaybeUninit},
    path::{Path, PathBuf},
    ptr,
};

use libloading::{Library, Symbol};
use thiserror::Error;
use tracing::{info, trace, warn};

use crate::protocol::{FastBootCommand, FastBootResponse, FastBootResponseParseError};

const ANDROID_USB_CLASS_ID: Guid = Guid {
    data1: 0xf72fe0d4,
    data2: 0xcbcb,
    data3: 0x407d,
    data4: [0x88, 0x14, 0x9e, 0xd6, 0x73, 0xd0, 0xdd, 0x6b],
};

const ADB_OPEN_ACCESS_TYPE_READ_WRITE: i32 = 0;
const ADB_OPEN_SHARING_MODE_READ_WRITE: i32 = 0;
const MAX_USBFS_BULK_SIZE: usize = 1024 * 1024;
const INITIAL_INTERFACE_BUFFER_SIZE: usize = 4096;
const READ_BUFFER_SIZE: usize = 4096;
const ERROR_INSUFFICIENT_BUFFER: i32 = 122;
const ERROR_NO_MORE_ITEMS: i32 = 259;

type AdbHandle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct AdbInterfaceInfo {
    class_id: Guid,
    flags: u32,
    device_name: [u16; 1],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct UsbDeviceDescriptor {
    b_length: u8,
    b_descriptor_type: u8,
    bcd_usb: u16,
    b_device_class: u8,
    b_device_sub_class: u8,
    b_device_protocol: u8,
    b_max_packet_size0: u8,
    id_vendor: u16,
    id_product: u16,
    bcd_device: u16,
    i_manufacturer: u8,
    i_product: u8,
    i_serial_number: u8,
    b_num_configurations: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct UsbInterfaceDescriptor {
    b_length: u8,
    b_descriptor_type: u8,
    b_interface_number: u8,
    b_alternate_setting: u8,
    b_num_endpoints: u8,
    b_interface_class: u8,
    b_interface_sub_class: u8,
    b_interface_protocol: u8,
    i_interface: u8,
}

// SAFETY: These `unsafe extern "C" fn` type aliases model the FFI function signatures exported by
// AdbWinApi.dll. Each corresponds to a known function exported by that DLL. The ABI must match
// exactly — any mismatch causes undefined behavior at the call site.
type AdbEnumInterfaces = unsafe extern "C" fn(Guid, bool, bool, bool) -> AdbHandle;
type AdbNextInterface = unsafe extern "C" fn(AdbHandle, *mut AdbInterfaceInfo, *mut u32) -> bool;
type AdbCreateInterfaceByName = unsafe extern "C" fn(*const u16) -> AdbHandle;
type AdbGetUsbDeviceDescriptor = unsafe extern "C" fn(AdbHandle, *mut UsbDeviceDescriptor) -> bool;
type AdbGetUsbInterfaceDescriptor =
    unsafe extern "C" fn(AdbHandle, *mut UsbInterfaceDescriptor) -> bool;
type AdbOpenDefaultBulkReadEndpoint = unsafe extern "C" fn(AdbHandle, i32, i32) -> AdbHandle;
type AdbOpenDefaultBulkWriteEndpoint = unsafe extern "C" fn(AdbHandle, i32, i32) -> AdbHandle;
type AdbReadEndpointSync = unsafe extern "C" fn(AdbHandle, *mut c_void, u32, *mut u32, u32) -> bool;
type AdbWriteEndpointSync =
    unsafe extern "C" fn(AdbHandle, *mut c_void, u32, *mut u32, u32) -> bool;
type AdbCloseHandle = unsafe extern "C" fn(AdbHandle) -> bool;

/// Probe detail emitted while discovering a Windows AdbWinApi backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdbWinApiProbeDetail {
    /// No `AdbWinApi.dll` was found in the search paths.
    DllMissing {
        /// Search paths that were checked.
        searched: Vec<PathBuf>,
    },
    /// Loading `AdbWinApi.dll` failed.
    DllLoadFailed {
        /// DLL path that failed to load.
        path: PathBuf,
        /// Loader error string.
        error: String,
    },
    /// The code is currently enumerating Android USB interfaces.
    EnumeratingInterfaces {
        /// DLL path that produced the probe event.
        source: PathBuf,
    },
    /// An Android USB interface was found.
    AndroidInterfaceFound {
        /// USB interface name.
        name: String,
    },
    /// A fastboot-capable interface was found.
    FastbootInterfaceFound {
        /// USB interface name.
        name: String,
    },
    /// An interface could not be opened.
    OpenInterfaceFailed {
        /// USB interface name.
        name: String,
        /// Open error string.
        error: String,
    },
    /// No Android USB interfaces were reported.
    NoAndroidInterface,
    /// Android USB interfaces were found, but none matched fastboot.
    NoFastbootInterface,
}

impl fmt::Display for AdbWinApiProbeDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DllMissing { searched } => {
                write!(
                    f,
                    "AdbWinApi.dll not found; searched {}",
                    searched
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Self::DllLoadFailed { path, error } => {
                write!(f, "failed to load {}: {error}", path.display())
            }
            Self::EnumeratingInterfaces { source } => write!(
                f,
                "enumerating Android USB interfaces via {}",
                source.display()
            ),
            Self::AndroidInterfaceFound { name } => write!(f, "found Android USB interface {name}"),
            Self::FastbootInterfaceFound { name } => write!(f, "found fastboot interface {name}"),
            Self::OpenInterfaceFailed { name, error } => {
                write!(f, "failed to open Android USB interface {name}: {error}")
            }
            Self::NoAndroidInterface => write!(f, "AdbWinApi enumerated no Android USB interfaces"),
            Self::NoFastbootInterface => write!(
                f,
                "AdbWinApi found Android USB interfaces but none matched fastboot"
            ),
        }
    }
}

/// Errors surfaced by the Windows AdbWinApi fastboot transport while issuing commands.
#[derive(Debug, Error)]
pub enum AdbWinApiFastbootError {
    /// Underlying I/O failure from the Windows API.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A fastboot command failed on the device.
    #[error("Fastboot client failure: {0}")]
    FastbootFailed(String),
    /// The device returned an unexpected response kind.
    #[error("Unexpected fastboot response")]
    FastbootUnexpectedReply,
    /// A response could not be parsed as a fastboot packet.
    #[error("Unknown fastboot response: {0}")]
    FastbootParseError(#[from] FastBootResponseParseError),
    /// A fastboot variable failed validation.
    #[error("Invalid fastboot variable {name}={value:?}: {reason}")]
    InvalidVariable {
        /// Variable name.
        name: &'static str,
        /// Returned value.
        value: String,
        /// Why the value was rejected.
        reason: String,
    },
    /// The device reported an unexpected byte count while streaming data.
    #[error("Incorrect data length: expected {expected}, got {actual}")]
    IncorrectDataLength {
        /// Actual bytes transferred.
        actual: u32,
        /// Expected bytes.
        expected: u32,
    },
}

/// Errors while opening the Windows AdbWinApi fastboot transport.
#[derive(Debug, Error)]
pub enum AdbWinApiFastbootOpenError {
    /// No `AdbWinApi.dll` was found.
    #[error("AdbWinApi.dll not found")]
    DllMissing {
        /// Search paths that were checked.
        searched: Vec<PathBuf>,
    },
    /// `AdbWinApi.dll` failed to load.
    #[error("failed to load {path}: {error}")]
    DllLoadFailed {
        /// DLL path that failed to load.
        path: PathBuf,
        /// Loader error string.
        error: String,
    },
    /// Interface enumeration failed unexpectedly.
    #[error("failed to enumerate Android USB interfaces from {path}: {error}")]
    InterfaceEnumerationFailed {
        /// DLL path that produced the failure.
        path: PathBuf,
        /// Enumeration error string.
        error: String,
    },
    /// No Android USB interfaces were reported.
    #[error("no Android USB interfaces reported by AdbWinApi")]
    NoAndroidInterface,
    /// No fastboot interface matched the Android USB interfaces.
    #[error("no fastboot interface matched Android USB interfaces")]
    NoFastbootInterface,
    /// Creating an interface handle failed.
    #[error("failed to open interface {name}: {error}")]
    OpenInterfaceFailed {
        /// USB interface name.
        name: String,
        /// Open error string.
        error: String,
    },
    /// Reading USB descriptors failed.
    #[error("failed to read USB descriptors for {name}: {error}")]
    DescriptorReadFailed {
        /// USB interface name.
        name: String,
        /// Descriptor read error string.
        error: String,
    },
    /// Opening the read or write endpoint failed.
    #[error("failed to open endpoints for {name}: {error}")]
    EndpointOpenFailed {
        /// USB interface name.
        name: String,
        /// Endpoint open error string.
        error: String,
    },
}

impl AdbWinApiFastbootOpenError {
    /// Return a structured probe detail for logging when one is available.
    pub fn detail(&self) -> Option<AdbWinApiProbeDetail> {
        Some(match self {
            Self::DllMissing { searched } => AdbWinApiProbeDetail::DllMissing {
                searched: searched.clone(),
            },
            Self::DllLoadFailed { path, error } => AdbWinApiProbeDetail::DllLoadFailed {
                path: path.clone(),
                error: error.clone(),
            },
            Self::InterfaceEnumerationFailed { path, .. } => {
                AdbWinApiProbeDetail::EnumeratingInterfaces {
                    source: path.clone(),
                }
            }
            Self::NoAndroidInterface => AdbWinApiProbeDetail::NoAndroidInterface,
            Self::NoFastbootInterface => AdbWinApiProbeDetail::NoFastbootInterface,
            Self::OpenInterfaceFailed { name, error } => {
                AdbWinApiProbeDetail::OpenInterfaceFailed {
                    name: name.clone(),
                    error: error.clone(),
                }
            }
            Self::DescriptorReadFailed { .. } | Self::EndpointOpenFailed { .. } => return None,
        })
    }
}

/// State for an open Windows AdbWinApi fastboot connection.
///
/// The transport keeps the loaded DLL and the underlying interface and endpoint handles alive for
/// the lifetime of the client.
#[derive(Debug)]
pub struct AdbWinApiFastboot {
    api: AdbApi,
    interface: AdbHandle,
    read_pipe: AdbHandle,
    write_pipe: AdbHandle,
    discovery: super::AdbWinApiDiscovery,
}

/// A queued Windows AdbWinApi data transfer.
pub struct DataDownload<'a> {
    fastboot: &'a mut AdbWinApiFastboot,
    size: u32,
    left: u32,
    current: Vec<u8>,
}

#[derive(Debug)]
struct AdbApi {
    _library: Library,
    enum_interfaces: AdbEnumInterfaces,
    next_interface: AdbNextInterface,
    create_interface_by_name: AdbCreateInterfaceByName,
    get_usb_device_descriptor: AdbGetUsbDeviceDescriptor,
    get_usb_interface_descriptor: AdbGetUsbInterfaceDescriptor,
    open_default_bulk_read_endpoint: AdbOpenDefaultBulkReadEndpoint,
    open_default_bulk_write_endpoint: AdbOpenDefaultBulkWriteEndpoint,
    read_endpoint_sync: AdbReadEndpointSync,
    write_endpoint_sync: AdbWriteEndpointSync,
    close_handle: AdbCloseHandle,
}

// SAFETY: `AdbWinApiFastboot` wraps Windows handle types (`AdbHandle = *mut c_void`) which are
// safe to send between threads. The handles are only used from synchronous methods and the
// `Drop` impl serializes cleanup. No thread-unsafe state is stored.
unsafe impl Send for AdbWinApiFastboot {}
// SAFETY: Same rationale as `Send` — the Windows handles support concurrent close/open
// operations and all mutability is behind `&mut self` at the Rust level, so shared
// references cannot race.
unsafe impl Sync for AdbWinApiFastboot {}

impl AdbWinApiFastboot {
    /// Open the first Windows AdbWinApi fastboot device that can be enumerated.
    pub fn open_first() -> Result<Self, AdbWinApiFastbootOpenError> {
        let discovery = discover_dlls()?;
        // SAFETY: `AdbApi::load` resolves function symbols from a dynamically-loaded DLL. The DLL
        // path comes from `discover_dlls()` which verifies the file exists. `load` returns an error
        // if the DLL can't be opened or symbols can't be resolved, so UB is contained.
        let api = unsafe { AdbApi::load(&discovery.adb_win_api)? };
        trace!("Loaded AdbWinApi from {}", discovery.adb_win_api.display());
        let mut saw_android = false;

        // SAFETY: `api.enum_interfaces` is a valid function pointer loaded from AdbWinApi.dll.
        // The function takes a GUID+flags and returns a handle (or null on failure). Null is
        // handled immediately after the call.
        let enum_handle = unsafe { (api.enum_interfaces)(ANDROID_USB_CLASS_ID, true, true, true) };
        if enum_handle.is_null() {
            return Err(AdbWinApiFastbootOpenError::NoAndroidInterface);
        }

        let mut entry_buffer = vec![0u8; INITIAL_INTERFACE_BUFFER_SIZE];
        loop {
            let mut entry_buffer_size = entry_buffer.len() as u32;
            // SAFETY: `api.next_interface` is a valid function pointer. The buffer is
            // heap-allocated and sized according to the most recent successful call. The function
            // writes into `entry_buffer` and updates `entry_buffer_size` on success or when it
            // reports that the buffer was too small.
            let has_next = unsafe {
                (api.next_interface)(
                    enum_handle,
                    entry_buffer.as_mut_ptr().cast::<AdbInterfaceInfo>(),
                    &mut entry_buffer_size,
                )
            };
            if !has_next {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(ERROR_INSUFFICIENT_BUFFER) => {
                        let required = entry_buffer_size as usize;
                        if required <= entry_buffer.len() {
                            unsafe { (api.close_handle)(enum_handle) };
                            return Err(AdbWinApiFastbootOpenError::InterfaceEnumerationFailed {
                                path: discovery.adb_win_api.clone(),
                                error: format!(
                                    "AdbNextInterface reported insufficient buffer but size stayed at {required}"
                                ),
                            });
                        }
                        entry_buffer.resize(required, 0);
                        continue;
                    }
                    Some(ERROR_NO_MORE_ITEMS) => break,
                    _ => {
                        unsafe { (api.close_handle)(enum_handle) };
                        return Err(AdbWinApiFastbootOpenError::InterfaceEnumerationFailed {
                            path: discovery.adb_win_api.clone(),
                            error: error.to_string(),
                        });
                    }
                }
            }

            // SAFETY: `entry_buffer` was populated by the preceding `next_interface` call, so
            // the returned byte count is authoritative for the entry. The helper validates that
            // the record is large enough before decoding the UTF-16 interface name.
            let entry_len = entry_buffer_size as usize;
            if entry_len > entry_buffer.len() {
                unsafe { (api.close_handle)(enum_handle) };
                return Err(AdbWinApiFastbootOpenError::InterfaceEnumerationFailed {
                    path: discovery.adb_win_api.clone(),
                    error: format!(
                        "AdbNextInterface reported {entry_len} bytes, but the buffer only held {} bytes",
                        entry_buffer.len()
                    ),
                });
            }
            let name = interface_name_from_entry(&entry_buffer[..entry_len]).map_err(|error| {
                AdbWinApiFastbootOpenError::InterfaceEnumerationFailed {
                    path: discovery.adb_win_api.clone(),
                    error,
                }
            })?;
            saw_android = true;
            trace!("AdbWinApi candidate interface: {name}");

            // SAFETY: `open_candidate` performs multiple FFI calls to open a USB interface,
            // create read/write endpoints, and validate the interface descriptor. Callers must
            // ensure `api` contains valid loaded function pointers and `name` is a valid
            // interface name from `next_interface`.
            match unsafe { Self::open_candidate(&api, &name) } {
                Ok((interface, read_pipe, write_pipe)) => {
                    // SAFETY: `enum_handle` is a valid handle returned by `enum_interfaces`.
                    // `close_handle` is the matching deallocator from the same DLL.
                    unsafe { (api.close_handle)(enum_handle) };
                    return Ok(Self {
                        api,
                        interface,
                        read_pipe,
                        write_pipe,
                        discovery,
                    });
                }
                Err(AdbWinApiFastbootOpenError::NoFastbootInterface) => continue,
                Err(AdbWinApiFastbootOpenError::DescriptorReadFailed { .. }) => continue,
                Err(AdbWinApiFastbootOpenError::OpenInterfaceFailed { .. }) => continue,
                Err(error) => {
                    // SAFETY: Same as above — `enum_handle` is still valid, needs cleanup.
                    unsafe { (api.close_handle)(enum_handle) };
                    return Err(error);
                }
            }
        }

        // SAFETY: After the loop, `enum_handle` is either still open (no match found) or already
        // closed (matched case above returns early). Here we always close it.
        unsafe { (api.close_handle)(enum_handle) };

        if !saw_android {
            Err(AdbWinApiFastbootOpenError::NoAndroidInterface)
        } else {
            Err(AdbWinApiFastbootOpenError::NoFastbootInterface)
        }
    }

    /// # Safety
    ///
    /// `api` must contain valid function pointers loaded from AdbWinApi.dll. `name` must be a
    /// valid USB interface name (obtained from `next_interface`). The caller is responsible for
    /// closing the returned handles via `api.close_handle`.
    unsafe fn open_candidate(
        api: &AdbApi,
        name: &str,
    ) -> Result<(AdbHandle, AdbHandle, AdbHandle), AdbWinApiFastbootOpenError> {
        let wide_name = to_wide(name);
        let interface = (api.create_interface_by_name)(wide_name.as_ptr());
        if interface.is_null() {
            return Err(AdbWinApiFastbootOpenError::OpenInterfaceFailed {
                name: name.to_string(),
                error: io::Error::last_os_error().to_string(),
            });
        }

        let mut device_desc = MaybeUninit::<UsbDeviceDescriptor>::zeroed();
        if !(api.get_usb_device_descriptor)(interface, device_desc.as_mut_ptr()) {
            (api.close_handle)(interface);
            return Err(AdbWinApiFastbootOpenError::DescriptorReadFailed {
                name: name.to_string(),
                error: io::Error::last_os_error().to_string(),
            });
        }

        let mut interface_desc = MaybeUninit::<UsbInterfaceDescriptor>::zeroed();
        if !(api.get_usb_interface_descriptor)(interface, interface_desc.as_mut_ptr()) {
            (api.close_handle)(interface);
            return Err(AdbWinApiFastbootOpenError::DescriptorReadFailed {
                name: name.to_string(),
                error: io::Error::last_os_error().to_string(),
            });
        }

        let interface_desc = interface_desc.assume_init();
        if !is_fastboot_interface(&interface_desc) {
            (api.close_handle)(interface);
            return Err(AdbWinApiFastbootOpenError::NoFastbootInterface);
        }

        let read_pipe = (api.open_default_bulk_read_endpoint)(
            interface,
            ADB_OPEN_ACCESS_TYPE_READ_WRITE,
            ADB_OPEN_SHARING_MODE_READ_WRITE,
        );
        if read_pipe.is_null() {
            let error = io::Error::last_os_error().to_string();
            (api.close_handle)(interface);
            return Err(AdbWinApiFastbootOpenError::EndpointOpenFailed {
                name: name.to_string(),
                error,
            });
        }

        let write_pipe = (api.open_default_bulk_write_endpoint)(
            interface,
            ADB_OPEN_ACCESS_TYPE_READ_WRITE,
            ADB_OPEN_SHARING_MODE_READ_WRITE,
        );
        if write_pipe.is_null() {
            let error = io::Error::last_os_error().to_string();
            (api.close_handle)(read_pipe);
            (api.close_handle)(interface);
            return Err(AdbWinApiFastbootOpenError::EndpointOpenFailed {
                name: name.to_string(),
                error,
            });
        }

        info!(
            "AdbWinApi opened fastboot interface {name} {:04x}:{:04x}",
            device_desc.assume_init().id_vendor,
            device_desc.assume_init().id_product
        );
        Ok((interface, read_pipe, write_pipe))
    }

    /// Return the DLL discovery information used to open this transport.
    pub fn discovery(&self) -> &super::AdbWinApiDiscovery {
        &self.discovery
    }

    fn parse_is_logical_value(value: &str) -> Result<bool, String> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("yes") {
            return Ok(true);
        }
        if value.eq_ignore_ascii_case("no") {
            return Ok(false);
        }
        Err(format!("unsupported logical partition value: {value}"))
    }

    fn parse_current_slot_value(value: &str) -> Result<&'static str, String> {
        let value = value.trim().trim_start_matches('_');
        if value.eq_ignore_ascii_case("a") {
            return Ok("a");
        }
        if value.eq_ignore_ascii_case("b") {
            return Ok("b");
        }
        Err(format!("unsupported slot value: {value}"))
    }

    fn write_all_sync(&mut self, mut data: &[u8]) -> Result<(), AdbWinApiFastbootError> {
        while !data.is_empty() {
            let want = data.len().min(MAX_USBFS_BULK_SIZE) as u32;
            let mut written = 0_u32;
            // SAFETY: `api.write_endpoint_sync` is a valid function pointer from AdbWinApi.dll.
            // `self.write_pipe` is a valid handle opened by `open_candidate`. `data.as_ptr()` is
            // a valid byte slice; the FFI will read `want` bytes from it. `written` receives the
            // actual count and is checked afterwards.
            let ok = unsafe {
                (self.api.write_endpoint_sync)(
                    self.write_pipe,
                    data.as_ptr() as *mut c_void,
                    want,
                    &mut written,
                    5000,
                )
            };
            if !ok || written == 0 {
                return Err(io::Error::last_os_error().into());
            }
            data = &data[written as usize..];
        }
        Ok(())
    }

    fn read_once_sync(&mut self) -> Result<Vec<u8>, AdbWinApiFastbootError> {
        let mut buf = vec![0u8; READ_BUFFER_SIZE];
        let mut read = 0_u32;
        // SAFETY: `api.read_endpoint_sync` is a valid function pointer from AdbWinApi.dll.
        // `self.read_pipe` is a valid handle. `buf` is a writable `Vec<u8>` whose pointer and
        // length are passed; the FFI writes up to `buf.len()` bytes into it. `read` captures
        // the actual byte count.
        let ok = unsafe {
            (self.api.read_endpoint_sync)(
                self.read_pipe,
                buf.as_mut_ptr().cast::<c_void>(),
                buf.len() as u32,
                &mut read,
                0,
            )
        };
        if !ok || read == 0 {
            return Err(io::Error::last_os_error().into());
        }
        buf.truncate(read as usize);
        Ok(buf)
    }

    fn send_command_sync<S: fmt::Display>(
        &mut self,
        cmd: FastBootCommand<S>,
    ) -> Result<(), AdbWinApiFastbootError> {
        self.write_all_sync(format!("{cmd}").as_bytes())
    }

    fn read_response_sync(&mut self) -> Result<FastBootResponse, AdbWinApiFastbootError> {
        let data = self.read_once_sync()?;
        Ok(FastBootResponse::from_bytes(&data)?)
    }

    fn handle_responses_sync(&mut self) -> Result<String, AdbWinApiFastbootError> {
        loop {
            let resp = self.read_response_sync()?;
            trace!("AdbWinApi response: {resp:?}");
            match resp {
                FastBootResponse::Info(_) => (),
                FastBootResponse::Text(_) => (),
                FastBootResponse::Data(_) => {
                    return Err(AdbWinApiFastbootError::FastbootUnexpectedReply)
                }
                FastBootResponse::Okay(value) => return Ok(value),
                FastBootResponse::Fail(fail) => {
                    return Err(AdbWinApiFastbootError::FastbootFailed(fail))
                }
            }
        }
    }

    fn execute_sync<S: fmt::Display>(
        &mut self,
        cmd: FastBootCommand<S>,
    ) -> Result<String, AdbWinApiFastbootError> {
        self.send_command_sync(cmd)?;
        self.handle_responses_sync()
    }

    /// Read a fastboot variable by name.
    pub async fn get_var(&mut self, var: &str) -> Result<String, AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::GetVar(var))
    }

    /// Read a fastboot variable if the device exposes it.
    ///
    /// Missing variables are treated as `Ok(None)` instead of an error.
    pub async fn get_var_optional(
        &mut self,
        var: &str,
    ) -> Result<Option<String>, AdbWinApiFastbootError> {
        self.send_command_sync(FastBootCommand::GetVar(var))?;
        let result = match self.handle_responses_sync() {
            Ok(value) => Ok(Some(value)),
            Err(AdbWinApiFastbootError::FastbootFailed(message))
                if crate::transport::is_missing_variable_message(&message) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        };
        result
    }

    /// Read and parse `max-download-size`.
    pub async fn max_download_size(&mut self) -> Result<u32, AdbWinApiFastbootError> {
        let value = self.get_var("max-download-size").await?;
        crate::operation::parse_max_download_size(&value).map_err(|err| {
            AdbWinApiFastbootError::InvalidVariable {
                name: "max-download-size",
                value,
                reason: err.to_string(),
            }
        })
    }

    /// Read and normalize the current A/B slot suffix.
    pub async fn current_slot(&mut self) -> Result<String, AdbWinApiFastbootError> {
        let value = self.get_var("current-slot").await?;
        Self::parse_current_slot_value(&value)
            .map(str::to_string)
            .map_err(|reason| AdbWinApiFastbootError::InvalidVariable {
                name: "current-slot",
                value,
                reason,
            })
    }

    /// Start a download transfer for the given byte size.
    pub async fn download(
        &mut self,
        size: u32,
    ) -> Result<DataDownload<'_>, AdbWinApiFastbootError> {
        self.send_command_sync(FastBootCommand::<&str>::Download(size))?;
        loop {
            match self.read_response_sync()? {
                FastBootResponse::Info(i) => info!("info: {i}"),
                FastBootResponse::Text(t) => info!("Text: {t}"),
                FastBootResponse::Data(expected) => {
                    return Ok(DataDownload {
                        fastboot: self,
                        size: expected,
                        left: expected,
                        current: Vec::with_capacity(MAX_USBFS_BULK_SIZE),
                    })
                }
                FastBootResponse::Okay(_) => {
                    return Err(AdbWinApiFastbootError::FastbootUnexpectedReply)
                }
                FastBootResponse::Fail(fail) => {
                    return Err(AdbWinApiFastbootError::FastbootFailed(fail))
                }
            }
        }
    }

    /// Flash the given target partition or image.
    pub async fn flash(&mut self, target: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::Flash(target))
            .map(|_| ())
    }

    /// Return whether the named partition is logical.
    pub async fn is_logical(&mut self, partition: &str) -> Result<bool, AdbWinApiFastbootError> {
        let Some(value) = self.get_var_optional(&format!("is-logical:{partition}")).await? else {
            return Ok(false);
        };
        Self::parse_is_logical_value(&value).map_err(|reason| {
            AdbWinApiFastbootError::InvalidVariable {
                name: "is-logical",
                value,
                reason,
            }
        })
    }

    /// Resize a logical partition.
    pub async fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::ResizeLogicalPartition { partition, size })
            .map(|_| ())
    }

    /// Send the `continue` fastboot command.
    pub async fn continue_boot(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::Continue)
            .map(|_| ())
    }

    /// Mark the given slot active.
    pub async fn set_active(&mut self, slot: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::SetActive(slot))
            .map(|_| ())
    }

    /// Erase the given target partition.
    pub async fn erase(&mut self, target: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::Erase(target))
            .map(|_| ())
    }

    /// Reboot the device.
    pub async fn reboot(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::Reboot)
            .map(|_| ())
    }

    /// Reboot into bootloader mode.
    pub async fn reboot_bootloader(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::RebootBootloader)
            .map(|_| ())
    }

    /// Power the device down.
    pub async fn power_down(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::Powerdown)
            .map(|_| ())
    }

    /// Reboot the device into the requested mode.
    pub async fn reboot_to(&mut self, mode: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::RebootTo(mode))
            .map(|_| ())
    }

    /// Reboot the device into fastboot mode.
    pub async fn reboot_fastboot(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.reboot_to("fastboot").await
    }

    /// Unlock the bootloader.
    pub async fn unlock_bootloader(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::FlashingUnlock)
            .map(|_| ())
    }

    /// Lock the bootloader.
    pub async fn lock_bootloader(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::FlashingLock)
            .map(|_| ())
    }

    /// Query all fastboot variables reported by the device.
    pub async fn get_all_vars(
        &mut self,
    ) -> Result<HashMap<String, String>, AdbWinApiFastbootError> {
        self.send_command_sync(FastBootCommand::GetVar("all"))?;
        let mut vars = HashMap::new();
        loop {
            match self.read_response_sync()? {
                FastBootResponse::Info(i) => {
                    let Some((key, value)) = i.rsplit_once(':') else {
                        warn!("Failed to parse variable: {i}");
                        continue;
                    };
                    vars.insert(key.trim().to_string(), value.trim().to_string());
                }
                FastBootResponse::Text(t) => info!("Text: {t}"),
                FastBootResponse::Data(_) => {
                    return Err(AdbWinApiFastbootError::FastbootUnexpectedReply)
                }
                FastBootResponse::Okay(_) => return Ok(vars),
                FastBootResponse::Fail(fail) => {
                    return Err(AdbWinApiFastbootError::FastbootFailed(fail))
                }
            }
        }
    }
}

impl Drop for AdbWinApiFastboot {
    fn drop(&mut self) {
        // SAFETY: Each handle was previously opened by `open_candidate` or
        // `AdbApi::load`. The `close_handle` function pointer is loaded from the same
        // AdbWinApi.dll that opened them. Handles are null-checked and set to null after
        // close to prevent double-free. Each handle is distinct, so order doesn't matter.
        unsafe {
            if !self.write_pipe.is_null() {
                (self.api.close_handle)(self.write_pipe);
                self.write_pipe = ptr::null_mut();
            }
            if !self.read_pipe.is_null() {
                (self.api.close_handle)(self.read_pipe);
                self.read_pipe = ptr::null_mut();
            }
            if !self.interface.is_null() {
                (self.api.close_handle)(self.interface);
                self.interface = ptr::null_mut();
            }
        }
    }
}

impl DataDownload<'_> {
    /// Expected transfer size in bytes.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Bytes still left to satisfy the requested transfer size.
    pub fn left(&self) -> u32 {
        self.left
    }

    /// Append bytes to the in-flight download buffer.
    pub async fn extend_from_slice(
        &mut self,
        mut data: &[u8],
    ) -> Result<(), AdbWinApiFastbootError> {
        self.update_size(data.len() as u32)?;
        loop {
            let left = MAX_USBFS_BULK_SIZE.saturating_sub(self.current.len());
            if left >= data.len() {
                self.current.extend_from_slice(data);
                break;
            }
            self.current.extend_from_slice(&data[..left]);
            self.flush_current()?;
            data = &data[left..];
        }
        Ok(())
    }

    /// Reserve writable space in the current download buffer.
    pub async fn get_mut_data(&mut self, max: usize) -> Result<&mut [u8], AdbWinApiFastbootError> {
        if self.current.len() == MAX_USBFS_BULK_SIZE {
            self.flush_current()?;
        }
        let left = MAX_USBFS_BULK_SIZE.saturating_sub(self.current.len());
        let size = left.min(max);
        self.update_size(size as u32)?;
        let start = self.current.len();
        self.current.resize(start + size, 0);
        Ok(&mut self.current[start..])
    }

    /// Finalize the transfer and wait for the final fastboot response.
    pub async fn finish(mut self) -> Result<(), AdbWinApiFastbootError> {
        if self.left != 0 {
            return Err(AdbWinApiFastbootError::IncorrectDataLength {
                expected: self.size,
                actual: self.size - self.left,
            });
        }
        self.flush_current()?;
        self.fastboot.handle_responses_sync().map(|_| ())
    }

    fn update_size(&mut self, size: u32) -> Result<(), AdbWinApiFastbootError> {
        if size > self.left {
            return Err(AdbWinApiFastbootError::IncorrectDataLength {
                expected: self.size,
                actual: size - self.left + self.size,
            });
        }
        self.left -= size;
        Ok(())
    }

    fn flush_current(&mut self) -> Result<(), AdbWinApiFastbootError> {
        if self.current.is_empty() {
            return Ok(());
        }
        self.fastboot.write_all_sync(&self.current)?;
        self.current.clear();
        Ok(())
    }
}

impl AdbApi {
    /// # Safety
    ///
    /// `path` must point to a valid AdbWinApi.dll that exports all the required function
    /// symbols (`AdbEnumInterfaces`, `AdbNextInterface`, etc.). If the DLL exports
    /// incompatible signatures, calling any method on the returned `AdbApi` is UB.
    unsafe fn load(path: &Path) -> Result<Self, AdbWinApiFastbootOpenError> {
        let library =
            Library::new(path).map_err(|error| AdbWinApiFastbootOpenError::DllLoadFailed {
                path: path.to_path_buf(),
                error: error.to_string(),
            })?;
        Ok(Self {
            enum_interfaces: *load_symbol(&library, b"AdbEnumInterfaces\0", path)?,
            next_interface: *load_symbol(&library, b"AdbNextInterface\0", path)?,
            create_interface_by_name: *load_symbol(&library, b"AdbCreateInterfaceByName\0", path)?,
            get_usb_device_descriptor: *load_symbol(
                &library,
                b"AdbGetUsbDeviceDescriptor\0",
                path,
            )?,
            get_usb_interface_descriptor: *load_symbol(
                &library,
                b"AdbGetUsbInterfaceDescriptor\0",
                path,
            )?,
            open_default_bulk_read_endpoint: *load_symbol(
                &library,
                b"AdbOpenDefaultBulkReadEndpoint\0",
                path,
            )?,
            open_default_bulk_write_endpoint: *load_symbol(
                &library,
                b"AdbOpenDefaultBulkWriteEndpoint\0",
                path,
            )?,
            read_endpoint_sync: *load_symbol(&library, b"AdbReadEndpointSync\0", path)?,
            write_endpoint_sync: *load_symbol(&library, b"AdbWriteEndpointSync\0", path)?,
            close_handle: *load_symbol(&library, b"AdbCloseHandle\0", path)?,
            _library: library,
        })
    }
}

/// # Safety
///
/// `library` must be a valid open `Library`. `symbol` must be a null-terminated byte string
/// naming an exported symbol from that library. The caller is responsible for ensuring the
/// symbol's actual type matches `T`.
unsafe fn load_symbol<'a, T>(
    library: &'a Library,
    symbol: &[u8],
    path: &Path,
) -> Result<Symbol<'a, T>, AdbWinApiFastbootOpenError> {
    library
        .get(symbol)
        .map_err(|error| AdbWinApiFastbootOpenError::DllLoadFailed {
            path: path.to_path_buf(),
            error: error.to_string(),
        })
}

fn is_fastboot_interface(desc: &UsbInterfaceDescriptor) -> bool {
    desc.b_interface_class == 0xff
        && desc.b_interface_sub_class == 0x42
        && desc.b_interface_protocol == 0x03
        && desc.b_num_endpoints == 2
}

fn discover_dlls() -> Result<super::AdbWinApiDiscovery, AdbWinApiFastbootOpenError> {
    let search_dirs = candidate_search_dirs();
    let mut searched = Vec::new();
    for dir in search_dirs {
        let path = dir.join("AdbWinApi.dll");
        searched.push(path.clone());
        if path.is_file() {
            let adb_win_usb_api = dir.join("AdbWinUsbApi.dll");
            return Ok(super::AdbWinApiDiscovery {
                adb_win_api: path,
                adb_win_usb_api: adb_win_usb_api.is_file().then_some(adb_win_usb_api),
            });
        }
    }
    Err(AdbWinApiFastbootOpenError::DllMissing { searched })
}

fn candidate_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            dirs.push(dir.join("resources"));
            dirs.push(dir.join("resources").join("windows"));
            dirs.push(dir.join("windows"));
        }
    }

    if let Some(paths) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&paths));
    }
    for name in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(home) = env::var_os(name) {
            dirs.push(PathBuf::from(home).join("platform-tools"));
        }
    }
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        dirs.push(
            PathBuf::from(local_app_data)
                .join("Android")
                .join("Sdk")
                .join("platform-tools"),
        );
    }
    normalize_candidate_search_dirs(dirs)
}

fn normalize_candidate_search_dirs(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort();
    dirs.dedup();
    dirs
}

fn interface_name_from_entry(entry: &[u8]) -> Result<String, String> {
    let min_size = size_of::<AdbInterfaceInfo>();
    if entry.len() < min_size {
        return Err(format!(
            "interface info shorter than minimum size: {} bytes",
            entry.len()
        ));
    }
    let name_offset = size_of::<Guid>() + size_of::<u32>();
    let name_bytes = &entry[name_offset..];
    if name_bytes.len() % 2 != 0 {
        return Err(format!(
            "interface name buffer had odd length: {} bytes",
            name_bytes.len()
        ));
    }

    let mut name_words = Vec::with_capacity(name_bytes.len() / 2);
    for chunk in name_bytes.chunks_exact(2) {
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        if value == 0 {
            return String::from_utf16(&name_words).map_err(|error| error.to_string());
        }
        name_words.push(value);
    }

    Err("unterminated UTF-16 interface name".to_string())
}

fn wide_slice_to_string(words: &[u16]) -> Result<String, String> {
    let Some(len) = words.iter().position(|word| *word == 0) else {
        return Err("unterminated UTF-16 string".to_string());
    };
    String::from_utf16(&words[..len]).map_err(|error| error.to_string())
}

fn to_wide(input: &str) -> Vec<u16> {
    input.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        candidate_search_dirs, is_fastboot_interface, normalize_candidate_search_dirs,
        wide_slice_to_string, AdbWinApiFastboot, AdbWinApiFastbootOpenError, AdbWinApiProbeDetail,
        UsbInterfaceDescriptor,
    };

    #[test]
    fn candidate_search_dirs_include_current_exe_directory() {
        let current_exe = std::env::current_exe().unwrap();
        let current_dir = current_exe.parent().unwrap().to_path_buf();

        let dirs = candidate_search_dirs();

        assert!(dirs.contains(&current_dir));
    }

    #[test]
    fn normalize_candidate_search_dirs_sorts_and_dedups() {
        let dirs = normalize_candidate_search_dirs(vec![
            PathBuf::from("/tmp/z"),
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/z"),
            PathBuf::from("/tmp/b"),
        ]);

        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/b"),
                PathBuf::from("/tmp/z"),
            ]
        );
    }

    #[test]
    fn parse_current_slot_value_accepts_common_variants() {
        assert_eq!(
            AdbWinApiFastboot::parse_current_slot_value("_A").unwrap(),
            "a"
        );
        assert_eq!(
            AdbWinApiFastboot::parse_current_slot_value(" b ").unwrap(),
            "b"
        );
        assert_eq!(
            AdbWinApiFastboot::parse_current_slot_value("__B").unwrap(),
            "b"
        );
    }

    #[test]
    fn parse_current_slot_value_rejects_unknown_values() {
        let error = AdbWinApiFastboot::parse_current_slot_value("other").unwrap_err();
        assert!(error.contains("unsupported slot value"));
    }

    #[test]
    fn wide_slice_to_string_stops_at_terminator_and_rejects_missing_null() {
        let decoded = wide_slice_to_string(&[0x41, 0x42, 0x00, 0x43]).unwrap();
        assert_eq!(decoded, "AB");

        let error = wide_slice_to_string(&[0x41, 0x42, 0x43]).unwrap_err();
        assert!(error.contains("unterminated"));
    }

    #[test]
    fn is_fastboot_interface_requires_the_expected_descriptor_shape() {
        let fastboot = UsbInterfaceDescriptor {
            b_length: 0,
            b_descriptor_type: 0,
            b_interface_number: 0,
            b_alternate_setting: 0,
            b_num_endpoints: 2,
            b_interface_class: 0xff,
            b_interface_sub_class: 0x42,
            b_interface_protocol: 0x03,
            i_interface: 0,
        };
        assert!(is_fastboot_interface(&fastboot));

        let mut not_fastboot = fastboot;
        not_fastboot.b_num_endpoints = 1;
        assert!(!is_fastboot_interface(&not_fastboot));
    }

    #[test]
    fn interface_enumeration_failure_reports_probe_detail() {
        let error = AdbWinApiFastbootOpenError::InterfaceEnumerationFailed {
            path: PathBuf::from(r"C:\Android\platform-tools\AdbWinApi.dll"),
            error: "boom".to_string(),
        };

        let detail = error.detail().unwrap();
        assert_eq!(
            detail,
            AdbWinApiProbeDetail::EnumeratingInterfaces {
                source: PathBuf::from(r"C:\Android\platform-tools\AdbWinApi.dll"),
            }
        );
        assert!(error
            .to_string()
            .contains("failed to enumerate Android USB interfaces"));
    }
}
