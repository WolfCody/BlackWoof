//! The only state BLACK WOOF keeps: the last playback (what was playing and where).
//! One small text file, written atomically (temp file, fsync, rename). Nothing else is
//! persisted: no history, no settings, no statistics.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

const FILE_NAME: &str = "last_playback.txt";
const TMP_NAME: &str = "last_playback.tmp";
const MAGIC: &str = "BLACKWOOF1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Last {
    pub uri: String,
    pub position_ms: u64,
    /// Kept only so the restored track can be shown without querying anything at start.
    pub title: String,
    pub artist: String,
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

pub fn save(dir: &Path, last: &Last) -> std::io::Result<()> {
    let text = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        MAGIC,
        one_line(&last.uri),
        last.position_ms,
        one_line(&last.title),
        one_line(&last.artist)
    );
    let tmp = dir.join(TMP_NAME);
    let mut file = File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, dir.join(FILE_NAME))
}

pub fn load(dir: &Path) -> Option<Last> {
    let text = fs::read_to_string(dir.join(FILE_NAME)).ok()?;
    let mut lines = text.lines();
    if lines.next()? != MAGIC {
        return None;
    }
    let uri = lines.next()?.to_string();
    let position_ms = lines.next()?.trim().parse().ok()?;
    let title = lines.next()?.to_string();
    let artist = lines.next().unwrap_or("").to_string();
    if uri.is_empty() {
        return None;
    }
    Some(Last { uri, position_ms, title, artist })
}
