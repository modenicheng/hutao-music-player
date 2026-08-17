use std::io;
use std::mem::size_of;
use std::ptr;
use std::time::Duration;

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

use super::BoxStream;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
};

/// Named-pipe endpoint scoped to the current Windows login session.
pub fn endpoint() -> String {
    let session = current_session_id().unwrap_or(0);
    let user = std::env::var("USERNAME")
        .unwrap_or_else(|_| "user".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    format!(r"\\.\pipe\hutao-music-player-{user}-{session}")
}

fn current_session_id() -> io::Result<u32> {
    let mut session = 0_u32;
    // SAFETY: `session` is a valid writable pointer for the duration of the call.
    let ok = unsafe {
        windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
            GetCurrentProcessId(),
            &mut session,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(session)
    }
}

pub struct Listener {
    endpoint: String,
    pending: Option<NamedPipeServer>,
}

impl Listener {
    pub async fn bind() -> io::Result<Self> {
        Self::bind_named(&endpoint()).await
    }

    pub async fn bind_named(endpoint: &str) -> io::Result<Self> {
        let pending = create_server(endpoint, true)?;
        Ok(Self {
            endpoint: endpoint.to_owned(),
            pending: Some(pending),
        })
    }

    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        let connected = self
            .pending
            .take()
            .expect("listener always retains one pending pipe instance");
        connected.connect().await?;
        let next = create_server(&self.endpoint, false)?;
        self.pending = Some(next);
        Ok(Box::new(connected))
    }
}

fn create_server(endpoint: &str, first: bool) -> io::Result<NamedPipeServer> {
    let mut security = PipeSecurity::for_current_user()?;
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // SAFETY: `security.attributes` and its descriptor remain valid for this
    // synchronous CreateNamedPipeW call. Windows copies the descriptor into the
    // created kernel object before the call returns.
    unsafe {
        options.create_with_security_attributes_raw(
            endpoint,
            (&mut security.attributes as *mut SECURITY_ATTRIBUTES).cast(),
        )
    }
}

struct PipeSecurity {
    attributes: SECURITY_ATTRIBUTES,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl PipeSecurity {
    fn for_current_user() -> io::Result<Self> {
        let sid = current_user_sid_string()?;
        let sddl = format!("D:P(A;;GA;;;{sid})");
        let wide = sddl
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and `descriptor` is writable. The
        // returned allocation is released with LocalFree in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            attributes: SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
            descriptor,
        })
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: descriptor was allocated by ConvertStringSecurityDescriptor...
            unsafe {
                LocalFree(self.descriptor);
            }
        }
    }
}

fn current_user_sid_string() -> io::Result<String> {
    let token = ProcessToken::open()?;
    let mut needed = 0_u32;
    // First call obtains the required TOKEN_USER buffer size.
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0_u8; needed as usize];
    // SAFETY: buffer has exactly the size requested by Windows and TOKEN_USER is
    // read only while the buffer remains alive.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    let mut string_sid = ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut string_sid) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut len = 0_usize;
    unsafe {
        while *string_sid.add(len) != 0 {
            len += 1;
        }
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(string_sid, len) })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    unsafe {
        LocalFree(string_sid.cast());
    }
    Ok(value)
}

struct ProcessToken(HANDLE);

impl ProcessToken {
    fn open() -> io::Result<Self> {
        let mut token = ptr::null_mut();
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(token))
        }
    }
}

impl Drop for ProcessToken {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub async fn connect() -> io::Result<BoxStream> {
    connect_named(&endpoint()).await
}

pub async fn connect_named(endpoint: &str) -> io::Result<BoxStream> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match ClientOptions::new().open(endpoint) {
            Ok(client) => return Ok(Box::new(client)),
            Err(error)
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}
