//! `mdbook`'s low level rendering interface.
//!
//! # Note
//!
//! You usually don't need to work with this module directly. If you want to
//! implement your own backend, then check out the [For Developers] section of
//! the user guide.
//!
//! The definition for [RenderContext] may be useful though.
//!
//! [For Developers]: https://rust-lang-nursery.github.io/mdBook/lib/index.html
//! [RenderContext]: struct.RenderContext.html

pub use self::html_handlebars::HtmlHandlebars;

mod html_handlebars;

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use serde_json;
use tempfile;
use shlex::Shlex;

use errors::*;
use config::Config;
use book::Book;

/// An arbitrary `mdbook` backend.
///
/// Although it's quite possible for you to import `mdbook` as a library and
/// provide your own renderer, there are two main renderer implementations that
/// 99% of users will ever use:
///
/// - [HtmlHandlebars] - the built-in HTML renderer
/// - [CmdRenderer] - a generic renderer which shells out to a program to do the
///   actual rendering
///
/// [HtmlHandlebars]: struct.HtmlHandlebars.html
/// [CmdRenderer]: struct.CmdRenderer.html
pub trait Renderer {
    /// The `Renderer`'s name.
    fn name(&self) -> &str;

    /// Invoke the `Renderer`, passing in all the necessary information for
    /// describing a book.
    fn render(&self, ctx: &RenderContext) -> Result<()>;
}

/// The context provided to all renderers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderContext {
    /// Which version of `mdbook` did this come from (as written in `mdbook`'s
    /// `Cargo.toml`). Useful if you know the renderer is only compatible with
    /// certain versions of `mdbook`.
    pub version: String,
    /// The book's root directory.
    pub root: PathBuf,
    /// A loaded representation of the book itself.
    pub book: Book,
    /// The loaded configuration file.
    pub config: Config,
    /// Where the renderer *must* put any build artefacts generated. To allow
    /// renderers to cache intermediate results, this directory is not
    /// guaranteed to be empty or even exist.
    pub destination: PathBuf,
}

impl RenderContext {
    /// Create a new `RenderContext`.
    pub(crate) fn new<P, Q>(root: P, book: Book, config: Config, destination: Q) -> RenderContext
    where
        P: Into<PathBuf>,
        Q: Into<PathBuf>,
    {
        RenderContext {
            book: book,
            config: config,
            version: env!("CARGO_PKG_VERSION").to_string(),
            root: root.into(),
            destination: destination.into(),
        }
    }

    /// Get the source directory's (absolute) path on disk.
    pub fn source_dir(&self) -> PathBuf {
        self.root.join(&self.config.book.src)
    }

    /// Load a `RenderContext` from its JSON representation.
    pub fn from_json<R: Read>(reader: R) -> Result<RenderContext> {
        serde_json::from_reader(reader).chain_err(|| "Unable to deserialize the `RenderContext`")
    }
}

/// A generic renderer which will shell out to an arbitrary executable.
///
/// # Rendering Protocol
///
/// When the renderer's `render()` method is invoked, `CmdRenderer` will spawn
/// the `cmd` as a subprocess. The `RenderContext` is passed to the subprocess
/// as a JSON string (using `serde_json`).
///
/// > **Note:** The command used doesn't necessarily need to be a single
/// > executable (i.e. `/path/to/renderer`). The `cmd` string lets you pass
/// > in command line arguments, so there's no reason why it couldn't be
/// > `python /path/to/renderer --from mdbook --to epub`.
///
/// Anything the subprocess writes to `stdin` or `stdout` will be passed through
/// to the user. While this gives the renderer maximum flexibility to output
/// whatever it wants, to avoid spamming users it is recommended to avoid
/// unnecessary output.
///
/// To help choose the appropriate output level, the `RUST_LOG` environment
/// variable will be passed through to the subprocess, if set.
///
/// If the subprocess wishes to indicate that rendering failed, it should exit
/// with a non-zero return code.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdRenderer {
    name: String,
    cmd: String,
}

impl CmdRenderer {
    /// Create a new `CmdRenderer` which will invoke the provided `cmd` string.
    pub fn new(name: String, cmd: String) -> CmdRenderer {
        CmdRenderer { name, cmd }
    }

    fn compose_command(&self) -> Result<Command> {
        let mut words = Shlex::new(&self.cmd);
        let executable = match words.next() {
            Some(e) => e,
            None => bail!("Command string was empty"),
        };

        let mut cmd = Command::new(executable);

        for arg in words {
            cmd.arg(arg);
        }

        Ok(cmd)
    }
}

impl Renderer for CmdRenderer {
    fn name(&self) -> &str {
        &self.name
    }

    fn render(&self, ctx: &RenderContext) -> Result<()> {
        info!("Invoking the \"{}\" renderer", self.cmd);

        // We need to write the RenderContext to a temporary file here instead
        // of passing it in via a pipe. This prevents a race condition where
        // some quickly executing command (e.g. `/bin/true`) may exit before we
        // finish writing the render context (closing the stdin pipe and
        // throwing a write error).
        let mut temp = tempfile::tempfile().chain_err(|| "Unable to create a temporary file")?;
        serde_json::to_writer(&mut temp, &ctx)
            .chain_err(|| "Unable to serialize the RenderContext")?;

        let status = self.compose_command()?
            .stdin(temp)
            .current_dir(&ctx.destination)
            .status()
            .chain_err(|| "Unable to start the renderer")?;

        trace!("{} exited with output: {:?}", self.cmd, status);

        if !status.success() {
            error!("Renderer exited with non-zero return code.");
            bail!("The \"{}\" renderer failed", self.cmd);
        } else {
            Ok(())
        }
    }
}
