use crate::adapters::{EnvironmentVariable, SystemEnvironment};
use crate::python_urlopen::Opener;
#[cfg(any(test, not(any(target_os = "macos", windows))))]
use crate::python_urlopen_proxy::NoSystemProxies;
use crate::python_urlopen_proxy::{Proxies, SystemProxies};
#[cfg(windows)]
use crate::python_urlopen_registry::{InternetSettingsKey, Registry};
#[cfg(target_os = "macos")]
use crate::python_urlopen_sysconf::{DynamicStore, Sysconf};
use crate::python_urlopen_tls::UrlopenTls;
use crate::{PythonString, RemoteSchemaError, RemoteSchemaRetriever, UrlopenRetrieverOptions};
use std::fmt;
use std::io::Read;
use std::sync::{Arc, LazyLock, OnceLock};

/// The [`RemoteSchemaRetriever`] the Python dialect uses unless another is set: a port of CPython 3.12
/// `urllib.request.urlopen` with the default opener, which reads `http`, `https`, `data` and `file` URLs, follows
/// redirects and honours the `*_proxy` and `no_proxy` environment variables as Python does, with the same exception
/// classes and texts. When no proxy variable is set, the proxies and the exceptions of the macOS system configuration
/// or of the Windows registry apply, as in Python.
///
/// HTTPS checks the certificate chain and the host name against the operating system roots, or the certificates of
/// [`UrlopenRetrieverOptions::trusted_certificates`], or of `SSL_CERT_FILE` and `SSL_CERT_DIR` when either is set.
/// The proxies and the HTTPS context are built on the first retrieval, whatever its scheme, as `urlopen` builds them
/// with its opener: the proxies and the trust variables are read from the environment then, and so is the
/// `SSL_CERT_FILE` file. `no_proxy`, and the exceptions of the system settings while no proxy variable is set, are
/// read again on each request. An `ftp` URL fails once its server accepts the connection.
pub struct UrlopenRetriever {
    options: UrlopenRetrieverOptions,
    environment: Box<dyn EnvironmentVariable + Send + Sync>,
    system: Box<dyn SystemProxies>,
    proxies: OnceLock<Proxies>,
    tls: OnceLock<Result<UrlopenTls, RemoteSchemaError>>,
}

impl fmt::Debug for UrlopenRetriever {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UrlopenRetriever")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Default for UrlopenRetriever {
    fn default() -> Self {
        Self::new(UrlopenRetrieverOptions::default())
    }
}

impl UrlopenRetriever {
    pub fn new(options: UrlopenRetrieverOptions) -> Self {
        Self {
            options,
            environment: Box::new(SystemEnvironment),
            system: system_proxies(),
            proxies: OnceLock::new(),
            tls: OnceLock::new(),
        }
    }

    /// The retriever every server shares unless it is given another, as `urlopen` builds one opener per process.
    pub(crate) fn shared() -> Arc<dyn RemoteSchemaRetriever> {
        static SHARED: LazyLock<Arc<dyn RemoteSchemaRetriever>> =
            LazyLock::new(|| Arc::new(UrlopenRetriever::default()));
        Arc::clone(&SHARED)
    }

    /// Reads the proxy variables from `environment` in place of the process environment, and no system settings.
    #[cfg(test)]
    pub(crate) fn with_environment(mut self, environment: impl EnvironmentVariable + Send + Sync + 'static) -> Self {
        self.environment = Box::new(environment);
        self.system = Box::new(NoSystemProxies);
        self
    }

    /// Reads the system settings from `system` in place of the platform.
    #[cfg(test)]
    pub(crate) fn with_system_proxies(mut self, system: impl SystemProxies + 'static) -> Self {
        self.system = Box::new(system);
        self
    }

    /// What `getproxies()` answers when the opener is built, on the first retrieval.
    fn proxies(&self) -> &Proxies {
        self.proxies
            .get_or_init(|| Proxies::from_platform(self.environment.as_ref(), self.system.as_ref()))
    }

    /// What `HTTPSHandler()` builds with the opener, on the first retrieval.
    fn tls(&self) -> Result<&UrlopenTls, RemoteSchemaError> {
        self.tls
            .get_or_init(|| {
                UrlopenTls::from_environment(self.options.trusted_certificates.as_deref(), self.environment.as_ref())
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl RemoteSchemaRetriever for UrlopenRetriever {
    fn open(&self, uri: &str) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        self.open_python(&uri.into())
    }

    /// `urlopen` of the URI as Python holds it: a `data:` URL holding a surrogate only in its media type opens, as
    /// does a `file:` path whose surrogates escape bytes on Unix or are UTF-16 code units on Windows.
    fn open_python(&self, uri: &PythonString) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        let proxies = self.proxies();
        self.tls()?;
        let tls = || self.tls();
        let opener = Opener {
            proxies,
            environment: self.environment.as_ref(),
            system: self.system.as_ref(),
            tls: &tls,
        };
        let body: Box<dyn Read + '_> = opener.urlopen_text(uri)?;
        Ok(body)
    }
}

/// The system settings of the platform: the macOS system configuration.
#[cfg(target_os = "macos")]
fn system_proxies() -> Box<dyn SystemProxies> {
    Box::new(Sysconf(DynamicStore))
}

/// The system settings of the platform: the `Internet Settings` key of the current user in the Windows registry.
#[cfg(windows)]
fn system_proxies() -> Box<dyn SystemProxies> {
    Box::new(Registry(InternetSettingsKey))
}

/// The system settings of the platform: none outside macOS and Windows.
#[cfg(not(any(target_os = "macos", windows)))]
fn system_proxies() -> Box<dyn SystemProxies> {
    Box::new(NoSystemProxies)
}

#[cfg(test)]
#[path = "urlopen_retriever_tests.rs"]
mod tests;
