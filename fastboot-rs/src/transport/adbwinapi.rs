use std::{
    collections::HashMap,
    env,
    ffi::c_void,
    fmt, io,
    mem::MaybeUninit,
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
const READ_BUFFER_SIZE: usize = 4096;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdbWinApiProbeDetail {
    DllMissing { searched: Vec<PathBuf> },
    DllLoadFailed { path: PathBuf, error: String },
    EnumeratingInterfaces { source: PathBuf },
    AndroidInterfaceFound { name: String },
    FastbootInterfaceFound { name: String },
    OpenInterfaceFailed { name: String, error: String },
    NoAndroidInterface,
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

#[derive(Debug, Error)]
pub enum AdbWinApiFastbootError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Fastboot client failure: {0}")]
    FastbootFailed(String),
    #[error("Unexpected fastboot response")]
    FastbootUnexpectedReply,
    #[error("Unknown fastboot response: {0}")]
    FastbootParseError(#[from] FastBootResponseParseError),
    #[error("Invalid fastboot variable {name}={value:?}: {reason}")]
    InvalidVariable {
        name: &'static str,
        value: String,
        reason: String,
    },
    #[error("Incorrect data length: expected {expected}, got {actual}")]
    IncorrectDataLength { actual: u32, expected: u32 },
}

#[derive(Debug, Error)]
pub enum AdbWinApiFastbootOpenError {
    #[error("AdbWinApi.dll not found")]
    DllMissing { searched: Vec<PathBuf> },
    #[error("failed to load {path}: {error}")]
    DllLoadFailed { path: PathBuf, error: String },
    #[error("no Android USB interfaces reported by AdbWinApi")]
    NoAndroidInterface,
    #[error("no fastboot interface matched Android USB interfaces")]
    NoFastbootInterface,
    #[error("failed to open interface {name}: {error}")]
    OpenInterfaceFailed { name: String, error: String },
    #[error("failed to read USB descriptors for {name}: {error}")]
    DescriptorReadFailed { name: String, error: String },
    #[error("failed to open endpoints for {name}: {error}")]
    EndpointOpenFailed { name: String, error: String },
}

impl AdbWinApiFastbootOpenError {
    pub fn detail(&self) -> Option<AdbWinApiProbeDetail> {
        Some(match self {
            Self::DllMissing { searched } => AdbWinApiProbeDetail::DllMissing {
                searched: searched.clone(),
            },
            Self::DllLoadFailed { path, error } => AdbWinApiProbeDetail::DllLoadFailed {
                path: path.clone(),
                error: error.clone(),
            },
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

pub struct AdbWinApiFastboot {
    api: AdbApi,
    interface: AdbHandle,
    read_pipe: AdbHandle,
    write_pipe: AdbHandle,
    discovery: super::AdbWinApiDiscovery,
}

pub struct DataDownload<'a> {
    fastboot: &'a mut AdbWinApiFastboot,
    size: u32,
    left: u32,
    current: Vec<u8>,
}

unsafe impl Send for AdbWinApiFastboot {}
unsafe impl Sync for AdbWinApiFastboot {}

impl AdbWinApiFastboot {
    pub fn open_first() -> Result<Self, AdbWinApiFastbootOpenError> {
        let discovery = discover_dlls()?;
        let api = unsafe { AdbApi::load(&discovery.adb_win_api)? };
        trace!("Loaded AdbWinApi from {}", discovery.adb_win_api.display());
        let mut saw_android = false;

        let enum_handle = unsafe { (api.enum_interfaces)(ANDROID_USB_CLASS_ID, true, true, true) };
        if enum_handle.is_null() {
            return Err(AdbWinApiFastbootOpenError::NoAndroidInterface);
        }

        let mut entry_buffer = vec![0u8; 4096];
        loop {
            let mut entry_buffer_size = entry_buffer.len() as u32;
            let has_next = unsafe {
                (api.next_interface)(
                    enum_handle,
                    entry_buffer.as_mut_ptr().cast::<AdbInterfaceInfo>(),
                    &mut entry_buffer_size,
                )
            };
            if !has_next {
                break;
            }

            let name = unsafe {
                let info = entry_buffer.as_ptr().cast::<AdbInterfaceInfo>();
                wide_ptr_to_string((*info).device_name.as_ptr())
            };
            saw_android = true;
            trace!("AdbWinApi candidate interface: {name}");

            match unsafe { Self::open_candidate(&api, &name) } {
                Ok((interface, read_pipe, write_pipe)) => {
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
                    unsafe { (api.close_handle)(enum_handle) };
                    return Err(error);
                }
            }
        }

        unsafe { (api.close_handle)(enum_handle) };

        if !saw_android {
            Err(AdbWinApiFastbootOpenError::NoAndroidInterface)
        } else {
            Err(AdbWinApiFastbootOpenError::NoFastbootInterface)
        }
    }

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

    pub fn discovery(&self) -> &super::AdbWinApiDiscovery {
        &self.discovery
    }

    fn parse_is_logical_value(value: &str) -> Result<bool, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "yes" => Ok(true),
            "no" => Ok(false),
            other => Err(format!("unsupported logical partition value: {other}")),
        }
    }

    fn write_all_sync(&mut self, mut data: &[u8]) -> Result<(), AdbWinApiFastbootError> {
        while !data.is_empty() {
            let want = data.len().min(MAX_USBFS_BULK_SIZE) as u32;
            let mut written = 0_u32;
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

    pub async fn get_var(&mut self, var: &str) -> Result<String, AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::GetVar(var))
    }

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

    pub async fn current_slot(&mut self) -> Result<String, AdbWinApiFastbootError> {
        let value = self.get_var("current-slot").await?;
        match value.trim().trim_start_matches('_').to_lowercase().as_str() {
            "a" => Ok("a".to_string()),
            "b" => Ok("b".to_string()),
            other => Err(AdbWinApiFastbootError::InvalidVariable {
                name: "current-slot",
                value,
                reason: format!("unsupported slot value: {other}"),
            }),
        }
    }

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

    pub async fn flash(&mut self, target: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::Flash(target))
            .map(|_| ())
    }

    pub async fn is_logical(&mut self, partition: &str) -> Result<bool, AdbWinApiFastbootError> {
        let value = match self.get_var(&format!("is-logical:{partition}")).await {
            Ok(value) => value,
            Err(AdbWinApiFastbootError::FastbootFailed(message))
                if message.to_ascii_lowercase().contains("variable not found") =>
            {
                return Ok(false)
            }
            Err(error) => return Err(error),
        };
        Self::parse_is_logical_value(&value).map_err(|reason| {
            AdbWinApiFastbootError::InvalidVariable {
                name: "is-logical",
                value,
                reason,
            }
        })
    }

    pub async fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::ResizeLogicalPartition { partition, size })
            .map(|_| ())
    }

    pub async fn continue_boot(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::Continue)
            .map(|_| ())
    }

    pub async fn set_active(&mut self, slot: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::SetActive(slot))
            .map(|_| ())
    }

    pub async fn erase(&mut self, target: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::Erase(target))
            .map(|_| ())
    }

    pub async fn reboot(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::Reboot)
            .map(|_| ())
    }

    pub async fn reboot_bootloader(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::RebootBootloader)
            .map(|_| ())
    }

    pub async fn power_down(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::Powerdown)
            .map(|_| ())
    }

    pub async fn reboot_to(&mut self, mode: &str) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::RebootTo(mode))
            .map(|_| ())
    }

    pub async fn reboot_fastboot(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.reboot_to("fastboot").await
    }

    pub async fn unlock_bootloader(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::FlashingUnlock)
            .map(|_| ())
    }

    pub async fn lock_bootloader(&mut self) -> Result<(), AdbWinApiFastbootError> {
        self.execute_sync(FastBootCommand::<&str>::FlashingLock)
            .map(|_| ())
    }

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
    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn left(&self) -> u32 {
        self.left
    }

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
    dirs.sort();
    dirs.dedup();
    dirs
}

unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}

fn to_wide(input: &str) -> Vec<u16> {
    input.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::candidate_search_dirs;

    #[test]
    fn candidate_search_dirs_include_current_exe_directory() {
        let current_exe = std::env::current_exe().unwrap();
        let current_dir = current_exe.parent().unwrap().to_path_buf();

        let dirs = candidate_search_dirs();

        assert!(dirs.contains(&current_dir));
    }
}
