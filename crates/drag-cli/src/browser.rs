//! Shared boundary for local browser side effects.

use std::io;

pub(crate) trait BrowserLauncher: Send + Sync {
    fn open(&self, url: &str) -> io::Result<()>;
}

pub(crate) struct SystemBrowserLauncher;

impl BrowserLauncher for SystemBrowserLauncher {
    fn open(&self, url: &str) -> io::Result<()> {
        webbrowser::open(url)
    }
}

#[cfg(test)]
pub(crate) struct NoopBrowserLauncher;

#[cfg(test)]
impl BrowserLauncher for NoopBrowserLauncher {
    fn open(&self, _url: &str) -> io::Result<()> {
        Ok(())
    }
}
