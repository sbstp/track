use std::{
    ffi::OsString,
    fs::{self, DirBuilder},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, bail};
use clap::*;
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

    fn add(&self, path: PathBuf) -> anyhow::Result<()> {
        let path_bytes = path.as_os_str().as_bytes();
        self.handle
            .execute("INSERT INTO paths (path) VALUES (?)", [path_bytes])?;
        Ok(())
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

    fn rm(&self, path: PathBuf) -> anyhow::Result<()> {
        let path_bytes = path.as_os_str().as_bytes();
        self.handle.execute("DELETE FROM paths WHERE path = ?", [path_bytes])?;
        Ok(())
    }
}

fn find_matches(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut matches = Vec::new();
    for path in paths {
        let walker = WalkDir::new(path)
            .into_iter()
            .filter_entry(|d| !d.file_type().is_dir() || d.file_name() != ".git");
        for entry in walker {
            let entry = entry?;
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
        if child.file_name() != ".git" || !child.file_type()?.is_dir() {
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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let paths_db = PathsDB::open()?;
    match args {
        Args::Add { paths } => {
            for path in paths {
                let path = path.canonicalize()?;
                paths_db.add(path)?;
            }
        }
        Args::Ls => {
            for path in paths_db.list()? {
                println!("{}", path.display())
            }
        }
        Args::Rm { paths } => {
            for path in paths {
                let path = path.canonicalize()?;
                paths_db.rm(path)?;
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
            let root = path.canonicalize()?;
            let paths = paths_db.list()?;
            let matches = find_matches(&paths)?;
            match kind {
                ExportKind::Dir => {
                    clean_dir(&root)?;
                    export_dir(root, &matches)?;
                }
                ExportKind::Tar => todo!(),
                ExportKind::Zip => todo!(),
            }
        }
    }
    Ok(())
}
