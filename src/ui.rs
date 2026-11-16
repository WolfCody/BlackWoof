//! The text interface, composed in Rust. Java is a thin renderer: it draws the lines below
//! and reports which button id was pressed. Language and text color live only in the current
//! session and are never saved.
//!
//! Screen format, one line per row, fields separated by `|`:
//!   `S|gray` or `S|red`        text color for the whole screen
//!   `K|player` or `K|library`  which screen this is (for the Back key)
//!   `T|text`                   plain text
//!   `B|id|text`                one pressable text
//!   `H|id|text|id|text|...`    pressable texts side by side
//!   `LIST|n`                   a list of n rows; row i is asked with the item call

use crate::error::ErrorKind;
use crate::playlist::{Repeat, Track};

pub const ID_PREV: i32 = 1;
pub const ID_PLAY_PAUSE: i32 = 2;
pub const ID_NEXT: i32 = 3;
pub const ID_SHUFFLE: i32 = 4;
pub const ID_REPEAT: i32 = 5;
pub const ID_MUSIC: i32 = 6;
pub const ID_LANG: i32 = 7;
pub const ID_RED: i32 = 8;
pub const ID_REFRESH: i32 = 9;
pub const ID_BACK: i32 = 10;
/// List row `i` is reported as `ID_ITEM_BASE + i`.
pub const ID_ITEM_BASE: i32 = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Player,
    Library,
}

/// The one-line message under the controls.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Note {
    None,
    NoTrack,
    Empty,
    Denied,
    Unavailable,
    Error(ErrorKind),
}

pub struct View<'a> {
    pub screen: Screen,
    pub lang: Lang,
    pub red: bool,
    pub title: &'a str,
    pub artist: &'a str,
    pub playing: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub note: Note,
    pub list_len: usize,
}

const RULE: &str = "------------------------";

fn clean(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

fn pick<'a>(lang: Lang, ru: &'a str, en: &'a str) -> &'a str {
    match lang {
        Lang::Ru => ru,
        Lang::En => en,
    }
}

pub fn note_text(note: Note, lang: Lang) -> String {
    let text = match note {
        Note::None => "",
        Note::NoTrack => pick(lang, "Выберите трек в разделе МУЗЫКА", "Choose a track in MUSIC"),
        Note::Empty => pick(lang, "Музыка не найдена", "No music found"),
        Note::Denied => pick(
            lang,
            "Нет доступа к музыке. Нажмите ОБНОВИТЬ, чтобы повторить.",
            "No access to music. Press REFRESH to try again.",
        ),
        Note::Unavailable => pick(lang, "Файл недоступен", "File is not available"),
        Note::Error(kind) => match kind {
            ErrorKind::InvalidFd | ErrorKind::Io => pick(lang, "Файл недоступен", "File is not available"),
            ErrorKind::UnknownFormat => pick(lang, "Неизвестный формат", "Unknown format"),
            ErrorKind::NoAudio => pick(lang, "В файле нет звука", "No audio in the file"),
            ErrorKind::Unsupported => pick(lang, "Формат не поддерживается", "Format is not supported"),
            ErrorKind::Corrupt => pick(lang, "Файл повреждён", "File is damaged"),
            ErrorKind::OutputOpen | ErrorKind::OutputControl | ErrorKind::OutputWrite => {
                pick(lang, "Ошибка вывода звука", "Audio output error")
            }
            ErrorKind::Internal => pick(lang, "Внутренняя ошибка", "Internal error"),
        },
    };
    text.to_string()
}

/// Text of one list row: "Title — Artist"; missing pieces become "Unknown".
pub fn item_text(track: &Track, lang: Lang) -> String {
    let unknown = pick(lang, "Неизвестно", "Unknown");
    let title = if track.title.is_empty() { unknown } else { track.title.as_str() };
    let artist = if track.artist.is_empty() { unknown } else { track.artist.as_str() };
    clean(&format!("{} — {}", title, artist))
}

pub fn render(v: &View) -> String {
    let lang = v.lang;
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };

    line(format!("S|{}", if v.red { "red" } else { "gray" }));
    line(format!("K|{}", if v.screen == Screen::Player { "player" } else { "library" }));
    line("T|BLACK WOOF".to_string());
    line(format!("T|{}", RULE));

    match v.screen {
        Screen::Player => {
            let dash = "—";
            let title = if v.title.is_empty() { dash } else { v.title };
            let artist = if v.artist.is_empty() { dash } else { v.artist };
            line(format!("T|{} {}", pick(lang, "Трек:", "Track:"), clean(title)));
            line(format!("T|{} {}", pick(lang, "Исполнитель:", "Artist:"), clean(artist)));
            line("T|".to_string());
            line(format!(
                "H|{}|{}|{}|{}|{}|{}",
                ID_PREV,
                pick(lang, "[ ПРЕД ]", "[ PREV ]"),
                ID_PLAY_PAUSE,
                match (v.playing, lang) {
                    (true, Lang::Ru) => "[ ПАУЗА ]",
                    (true, Lang::En) => "[ PAUSE ]",
                    (false, Lang::Ru) => "[ ИГРАТЬ ]",
                    (false, Lang::En) => "[ PLAY ]",
                },
                ID_NEXT,
                pick(lang, "[ СЛЕД ]", "[ NEXT ]")
            ));
            line("T|".to_string());
            line(format!(
                "B|{}|{} {}",
                ID_SHUFFLE,
                pick(lang, "Перемешивание:", "Shuffle:"),
                match (v.shuffle, lang) {
                    (true, Lang::Ru) => "ВКЛ",
                    (true, Lang::En) => "ON",
                    (false, Lang::Ru) => "ВЫКЛ",
                    (false, Lang::En) => "OFF",
                }
            ));
            line(format!(
                "B|{}|{} {}",
                ID_REPEAT,
                pick(lang, "Повтор:", "Repeat:"),
                match (v.repeat, lang) {
                    (Repeat::Off, Lang::Ru) => "ВЫКЛ",
                    (Repeat::Off, Lang::En) => "OFF",
                    (Repeat::All, Lang::Ru) => "ВСЕ",
                    (Repeat::All, Lang::En) => "ALL",
                    (Repeat::One, Lang::Ru) => "ТРЕК",
                    (Repeat::One, Lang::En) => "ONE",
                }
            ));
            line(format!("T|{}", RULE));
            line(format!(
                "H|{}|{}|{}|{}|{}|{}",
                ID_MUSIC,
                pick(lang, "[ МУЗЫКА ]", "[ MUSIC ]"),
                ID_LANG,
                "[ RU/EN ]",
                ID_RED,
                pick(lang, "[ КРАСНЫЙ ]", "[ RED ]")
            ));
            let note = note_text(v.note, lang);
            if !note.is_empty() {
                line("T|".to_string());
                line(format!("T|{}", note));
            }
        }
        Screen::Library => {
            line(format!("T|{}", pick(lang, "МУЗЫКА", "MUSIC LIBRARY")));
            line(format!("T|{}", RULE));
            let note = note_text(v.note, lang);
            if !note.is_empty() {
                line(format!("T|{}", note));
            }
            if v.list_len > 0 {
                line(format!("LIST|{}", v.list_len));
            }
            line(format!("T|{}", RULE));
            line(format!(
                "H|{}|{}|{}|{}",
                ID_REFRESH,
                pick(lang, "[ ОБНОВИТЬ ]", "[ REFRESH ]"),
                ID_BACK,
                pick(lang, "[ НАЗАД ]", "[ BACK ]")
            ));
        }
    }
    out
}
