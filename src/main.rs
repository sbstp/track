use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder, File},
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, bail, Context};
use clap::*;
use path_absolutize::Absolutize;
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[clap(name = "track")]
enum Args {
    /// Add a new path to tracked paths.
    Add { paths: Vec<PathBuf> },

    /// List tracked paths.
    Ls,

    /// Remove a path from tracked paths.
    Rm { paths: Vec<PathBuf> },

    /// Automatically remove deleted or unaccessible paths.
    Prune,

    /// List all files matched by tracked paths.
    Matched,

    /// Export all the files matched by the tracked paths.
    Export {
        /// Kind of export, dir, tar or zip.
        kind: ExportKind,
        /// Path of directory or archive to export to.
        path: PathBuf,
    },
}

#[derive(Debug)]
enum ExportKind {
    Dir,
    Tar,
    Zip,
}

impl FromStr for ExportKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "dir" => ExportKind::Dir,
            "tar" => ExportKind::Tar,
            "zip" => ExportKind::Zip,
            _ => bail!("Unknown export kind {}", s),
        })
    }
}

pub struct PathsDB {
    handle: rusqlite::Connection,
}

impl PathsDB {
    fn open() -> anyhow::Result<PathsDB> {
        let mut db_path = dirs::config_dir().ok_or_else(|| anyhow!("couldn't get user config dir"))?;
        db_path.push("track.db");
        let handle = rusqlite::Connection::open(db_path)?;
        handle.execute_batch(include_str!("init.sql"))?;
        Ok(PathsDB { handle })
    }

    fn add(&self, path: &Path) -> anyhow::Result<()> {
        let path_bytes = path.as_os_str().as_bytes();
        match self.handle.execute("INSERT INTO paths (path) VALUES (?)", [path_bytes]) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => {
                eprintln!("Path already in database!");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    fn list(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut stmt = self.handle.prepare("SELECT path FROM paths ORDER BY path ASC")?;
        let mut rows = stmt.query([])?;
        let mut paths = Vec::new();
        while let Some(row) = rows.next()? {
            let path_bytes: Vec<u8> = row.get(0)?;
            paths.push(PathBuf::from(OsString::from_vec(path_bytes)));
        }
        Ok(paths)
    }

    fn rm(&self, path: &Path) -> anyhow::Result<()> {
        let path_bytes = path.as_os_str().as_bytes();
        self.handle.execute("DELETE FROM paths WHERE path = ?", [path_bytes])?;
        Ok(())
    }
}

trait DirEntryAdapter {
    fn file_name(&self) -> Cow<OsStr>;
    fn file_type(&self) -> io::Result<fs::FileType>;

    fn is_git_dir(&self) -> io::Result<bool> {
        Ok(self.file_name() == OsStr::new(".git") && self.file_type()?.is_dir())
    }
}

impl DirEntryAdapter for fs::DirEntry {
    fn file_name(&self) -> Cow<OsStr> {
        Cow::Owned(self.file_name())
    }

    fn file_type(&self) -> io::Result<fs::FileType> {
        self.file_type()
    }
}

impl DirEntryAdapter for walkdir::DirEntry {
    fn file_name(&self) -> Cow<OsStr> {
        Cow::Borrowed(self.file_name())
    }

    fn file_type(&self) -> io::Result<fs::FileType> {
        Ok(self.file_type())
    }
}

fn find_matches(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut matches = Vec::new();
    for path in paths {
        let walker = WalkDir::new(path)
            .into_iter()
            .filter_entry(|d| !d.is_git_dir().expect("could not if detect .git directory"));
        for entry in walker {
            let entry = entry.context(format!("error scanning path {}", path.display()))?;
            if entry.file_type().is_file() {
                matches.push(entry.into_path());
            }
        }
    }
    Ok(matches)
}

fn clean_dir(root: &Path) -> anyhow::Result<()> {
    let root_children = root.read_dir()?;
    for child in root_children {
        let child = child?;
        if !child.is_git_dir()? {
            fs::remove_dir_all(child.path())?;
        }
    }
    Ok(())
}

fn export_dir(root: PathBuf, matches: &[PathBuf]) -> anyhow::Result<()> {
    for mat in matches {
        let stem = mat.strip_prefix("/")?;
        let new_path = root.join(stem);
        DirBuilder::new()
            .recursive(true)
            .create(new_path.parent().expect("new path has no parent"))?;
        fs::copy(mat, new_path)?;
    }
    Ok(())
}

fn export_tar(root: PathBuf, matches: &[PathBuf]) -> anyhow::Result<()> {
    let output = File::create(root)?;
    let compressor = flate2::write::GzEncoder::new(output, flate2::Compression::default());
    let mut archiver = tar::Builder::new(compressor);

    for mat in matches {
        archiver
            .append_path_with_name(mat, mat.strip_prefix("/")?)
            .context(format!("could not add path {} to archive", mat.display()))?;
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let paths_db = PathsDB::open()?;
    match args {
        Args::Add { paths } => {
            for path in paths {
                let path = path.absolutize()?;
                paths_db.add(&path)?;
            }
        }
        Args::Ls => {
            for path in paths_db.list()? {
                println!("{}", path.display())
            }
        }
        Args::Rm { paths } => {
            for path in paths {
                let path = path.absolutize()?;
                paths_db.rm(&path)?;
            }
        }
        Args::Prune => {
            let paths = paths_db.list()?;
            for path in paths {
                if !path.exists() {
                    println!("Pruned {}", path.display());
                    paths_db.rm(&path)?;
                }
            }
        }
        Args::Matched => {
            let paths = paths_db.list()?;
            let matches = find_matches(&paths)?;
            for path in matches {
                println!("{}", path.display());
            }
        }
        Args::Export { kind, path } => {
            let paths = paths_db.list()?;
            let matches = find_matches(&paths)?;
            match kind {
                ExportKind::Dir => {
                    clean_dir(&path)?;
                    export_dir(path, &matches)?;
                }
                ExportKind::Tar => export_tar(path, &matches)?,
                ExportKind::Zip => todo!(),
            }
        }
    }
    Ok(())
}
