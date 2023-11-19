// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
use directories::UserDirs;
use eyre::{eyre, Result};
use time::OffsetDateTime;

use super::{get_histpath, unix_byte_lines, Importer, Loader};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Zsh {
    bytes: Vec<u8>,
}

fn default_histpath() -> Result<PathBuf> {
    // oh-my-zsh sets HISTFILE=~/.zhistory
    // zsh has no default value for this var, but uses ~/.zhistory.
    // we could maybe be smarter about this in the future :)
    let user_dirs: UserDirs = UserDirs::new().ok_or_else(|| eyre!("could not find user directories"))?;
    let home_dir: &std::path::Path = user_dirs.home_dir();

    let mut candidates: std::slice::Iter<'_, &str> = [".zhistory", ".zsh_history"].iter();
    loop {
        match candidates.next() {
            Some(candidate) => {
                let histpath: PathBuf = home_dir.join(candidate);
                if histpath.exists() {
                    break Ok(histpath);
                }
            }
            None => {
                break Err(eyre!(
                    "Could not find history file. Try setting and exporting $HISTFILE"
                ))
            }
        }
    }
}

#[async_trait]
impl Importer for Zsh {
    const NAME: &'static str = "zsh";

    async fn new() -> Result<Self> {
        let bytes: Vec<u8> = read_to_end(get_histpath(default_histpath)?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(super::count_lines(&self.bytes))
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let now: OffsetDateTime = OffsetDateTime::now_utc();
        let mut line: String = String::new();

        let mut counter: i64 = 0;
        for b in unix_byte_lines(&self.bytes) {
            let s: &str = match std::str::from_utf8(b) {
                Ok(s) => s,
                Err(_) => continue, // we can skip past things like invalid utf8
            };

            if let Some(s) = s.strip_suffix('\\') {
                line.push_str(s);
                line.push_str("\\\n");
            } else {
                line.push_str(s);
                let command: String = std::mem::take(&mut line);

                if let Some(command) = command.strip_prefix(": ") {
                    counter += 1;
                    h.push(parse_extended(command, counter)).await?;
                } else {
                    let offset: time::Duration = time::Duration::seconds(counter);
                    counter += 1;

                    let imported = History::import()
                        // preserve ordering
                        .timestamp(now - offset)
                        .command(command.trim_end().to_string());

                    h.push(imported.build().into()).await?;
                }
            }
        }

        Ok(())
    }
}

fn parse_extended(line: &str, counter: i64) -> History {
    let (time, duration) = line.split_once(':').unwrap();
    let (duration, command) = duration.split_once(';').unwrap();

    let time: OffsetDateTime = time
        .parse::<i64>()
        .ok()
        .and_then(|t: i64| OffsetDateTime::from_unix_timestamp(t).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
        + time::Duration::milliseconds(counter);

    // use nanos, because why the hell not? we won't display them.
    let duration = duration.parse::<i64>().map_or(-1, |t: i64| t * 1_000_000_000);

    let imported = History::import()
        .timestamp(time)
        .command(command.trim_end().to_string())
        .duration(duration);

    imported.build().into()
}

#[cfg(test)]
mod test {
    use itertools::assert_equal;

    use crate::import::tests::TestLoader;

    use super::*;

    #[test]
    fn test_parse_extended_simple() {
        let parsed: History = parse_extended("1613322469:0;cargo install atuin", 0);

        assert_eq!(parsed.command, "cargo install atuin");
        assert_eq!(parsed.duration, 0);
        assert_eq!(
            parsed.timestamp,
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );

        let parsed: History = parse_extended("1613322469:10;cargo install atuin;cargo update", 0);

        assert_eq!(parsed.command, "cargo install atuin;cargo update");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(
            parsed.timestamp,
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );

        let parsed: History = parse_extended("1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷", 0);

        assert_eq!(parsed.command, "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(
            parsed.timestamp,
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );

        let parsed: History = parse_extended("1613322469:10;cargo install \\n atuin\n", 0);

        assert_eq!(parsed.command, "cargo install \\n atuin");
        assert_eq!(parsed.duration, 10_000_000_000);
        assert_eq!(
            parsed.timestamp,
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );
    }

    #[tokio::test]
    async fn test_parse_file() {
        let bytes: Vec<u8> = r": 1613322469:0;cargo install atuin
: 1613322469:10;cargo install atuin; \
cargo update
: 1613322469:10;cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷
"
        .as_bytes()
        .to_owned();

        let mut zsh: Zsh = Zsh { bytes };
        assert_eq!(zsh.entries().await.unwrap(), 4);

        let mut loader: TestLoader = TestLoader::default();
        zsh.load(&mut loader).await.unwrap();

        assert_equal(
            loader.buf.iter().map(|h: &History| h.command.as_str()),
            [
                "cargo install atuin",
                "cargo install atuin; \\\ncargo update",
                "cargo :b̷i̶t̴r̵o̴t̴ ̵i̷s̴ ̷r̶e̵a̸l̷",
            ],
        );
    }
}
