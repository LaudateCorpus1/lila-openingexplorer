use std::{
    cmp::min, ffi::OsStr, fs::File, io, mem, num::Wrapping, path::PathBuf, thread, time::Duration,
};

use clap::Parser;
use pgn_reader::{BufferedReader, Color, Outcome, RawHeader, SanPlus, Skip, Visitor};
use rand::{distributions::OpenClosed01, rngs::SmallRng, Rng, SeedableRng};
use serde::Serialize;
use serde_with::{serde_as, DisplayFromStr, SpaceSeparator, StringWithSeparator};

#[derive(Debug, Serialize, Copy, Clone)]
#[serde(rename_all = "camelCase")]
enum Speed {
    UltraBullet,
    Bullet,
    Blitz,
    Rapid,
    Classical,
    Correspondence,
}

impl Speed {
    fn from_seconds_and_increment(seconds: u64, increment: u64) -> Speed {
        let total = seconds + 40 * increment;

        if total < 30 {
            Speed::UltraBullet
        } else if total < 180 {
            Speed::Bullet
        } else if total < 480 {
            Speed::Blitz
        } else if total < 1500 {
            Speed::Rapid
        } else if total < 21_600 {
            Speed::Classical
        } else {
            Speed::Correspondence
        }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Speed, ()> {
        if bytes == b"-" {
            return Ok(Speed::Correspondence);
        }

        let mut parts = bytes.splitn(2, |ch| *ch == b'+');
        let seconds = btoi::btou(parts.next().ok_or(())?).map_err(|_| ())?;
        let increment = btoi::btou(parts.next().ok_or(())?).map_err(|_| ())?;
        Ok(Speed::from_seconds_and_increment(seconds, increment))
    }
}

struct Batch {
    filename: PathBuf,
    games: Vec<Game>,
}

struct Importer {
    tx: crossbeam::channel::Sender<Batch>,
    filename: PathBuf,
    batch_size: usize,

    rng: SmallRng,
    current: Game,
    skip: bool,
    batch: Vec<Game>,
}

#[serde_as]
#[derive(Default, Serialize, Debug)]
struct Game {
    variant: Option<String>,
    speed: Option<Speed>,
    fen: Option<String>,
    id: Option<String>,
    date: Option<String>,
    white: Player,
    black: Player,
    #[serde_as(as = "Option<DisplayFromStr>")]
    winner: Option<Color>,
    #[serde_as(as = "StringWithSeparator<SpaceSeparator, SanPlus>")]
    moves: Vec<SanPlus>,
}

#[derive(Default, Serialize, Debug)]
struct Player {
    name: Option<String>,
    rating: Option<u16>,
}

impl Importer {
    fn new(
        tx: crossbeam::channel::Sender<Batch>,
        filename: PathBuf,
        batch_size: usize,
    ) -> Importer {
        Importer {
            tx,
            filename,
            batch_size,
            rng: SmallRng::from_seed([
                0x19, 0x29, 0xab, 0x17, 0xc6, 0xfa, 0xb0, 0xe9, 0x4b, 0x44, 0xd8, 0x07, 0x09, 0xbf,
                0x1d, 0x87, 0xbd, 0xd8, 0xb3, 0x2f, 0xe1, 0xe2, 0xa0, 0x1a, 0x9e, 0x30, 0x98, 0xd7,
                0xef, 0xd5, 0x7a, 0x1d,
            ]),
            current: Game::default(),
            skip: false,
            batch: Vec::with_capacity(batch_size),
        }
    }

    pub fn send(&mut self) {
        self.tx
            .send(Batch {
                filename: self.filename.clone(),
                games: mem::replace(&mut self.batch, Vec::with_capacity(self.batch_size)),
            })
            .expect("send");
    }
}

impl Visitor for Importer {
    type Result = ();

    fn begin_game(&mut self) {
        self.skip = false;
        self.current = Game::default();
    }

    fn header(&mut self, key: &[u8], value: RawHeader<'_>) {
        if key == b"White" {
            self.current.white.name = Some(value.decode_utf8().expect("White").into_owned());
        } else if key == b"Black" {
            self.current.black.name = Some(value.decode_utf8().expect("Black").into_owned());
        } else if key == b"WhiteElo" {
            if value.as_bytes() != b"?" {
                self.current.white.rating = Some(btoi::btoi(value.as_bytes()).expect("WhiteElo"));
            }
        } else if key == b"BlackElo" {
            if value.as_bytes() != b"?" {
                self.current.black.rating = Some(btoi::btoi(value.as_bytes()).expect("BlackElo"));
            }
        } else if key == b"TimeControl" {
            self.current.speed = Some(Speed::from_bytes(value.as_bytes()).expect("TimeControl"));
        } else if key == b"Variant" {
            self.current.variant = Some(value.decode_utf8().expect("Variant").into_owned());
        } else if key == b"Date" || key == b"UTCDate" {
            self.current.date = Some(value.decode_utf8().expect("Date").into_owned());
        } else if key == b"WhiteTitle" || key == b"BlackTitle" {
            if value.as_bytes() == b"BOT" {
                self.skip = true;
            }
        } else if key == b"Site" {
            self.current.id = Some(
                String::from_utf8(
                    value
                        .as_bytes()
                        .rsplitn(2, |ch| *ch == b'/')
                        .next()
                        .expect("Site")
                        .to_owned(),
                )
                .expect("Site"),
            );
        } else if key == b"Result" {
            match Outcome::from_ascii(value.as_bytes()) {
                Ok(outcome) => self.current.winner = outcome.winner(),
                Err(_) => self.skip = true,
            }
        } else if key == b"FEN" {
            if value.as_bytes() == b"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1" {
                // https://github.com/ornicar/lichess-db/issues/40
                self.current.fen = None;
            } else {
                self.current.fen = Some(value.decode_utf8().expect("FEN").into_owned());
            }
        }
    }

    fn end_headers(&mut self) -> Skip {
        let rating =
            (self.current.white.rating.unwrap_or(0) + self.current.black.rating.unwrap_or(0)) / 2;

        let standard = self
            .current
            .variant
            .as_ref()
            .map_or(true, |name| name == "Standard");

        let probability = if standard {
            match self.current.speed.unwrap_or(Speed::Correspondence) {
                Speed::Correspondence | Speed::Classical => 1.00,

                _ if rating >= 2500 => 1.00,

                Speed::Rapid if rating >= 2200 => 1.00,
                Speed::Rapid if rating >= 2000 => 0.83,
                Speed::Rapid if rating >= 1800 => 0.46,
                Speed::Rapid if rating >= 1600 => 0.39,

                Speed::Blitz if rating >= 2200 => 0.38,
                Speed::Blitz if rating >= 2000 => 0.18,
                Speed::Blitz if rating >= 1600 => 0.13,

                Speed::Bullet if rating >= 2200 => 0.48,
                Speed::Bullet if rating >= 2000 => 0.27,
                Speed::Bullet if rating >= 1800 => 0.19,
                Speed::Bullet if rating >= 1600 => 0.18,

                Speed::UltraBullet => 1.00,

                _ => 0.02,
            }
        } else {
            // variant games
            if rating >= 1600 {
                1.00
            } else {
                0.50
            }
        };

        let accept = min(
            self.current.white.rating.unwrap_or(0),
            self.current.black.rating.unwrap_or(0),
        ) >= 1501
            && probability >= self.rng.sample(OpenClosed01)
            && !self.skip;

        self.skip = !accept;
        Skip(self.skip)
    }

    fn san(&mut self, san: SanPlus) {
        self.current.moves.push(san);
    }

    fn begin_variation(&mut self) -> Skip {
        Skip(true) // stay in the mainline
    }

    fn end_game(&mut self) {
        if !self.skip {
            self.batch.push(mem::take(&mut self.current));

            if self.batch.len() >= self.batch_size {
                self.send();
            }
        }
    }
}

#[derive(Parser)]
struct Args {
    #[clap(long, default_value = "http://localhost:9002")]
    endpoint: String,
    #[clap(long, default_value = "200")]
    batch_size: usize,
    pgns: Vec<PathBuf>,
}

fn main() -> Result<(), io::Error> {
    let args = Args::parse();

    let (tx, rx) = crossbeam::channel::bounded::<Batch>(50);

    let bg = thread::spawn(move || {
        let mut spinner_idx = Wrapping(0);
        let spinner = &['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("client");

        while let Ok(batch) = rx.recv() {
            let res = client
                .put(format!("{}/import/lichess", args.endpoint))
                .json(&batch.games)
                .send()
                .expect("send batch");

            spinner_idx += Wrapping(1);

            println!(
                "{} {:?}: {}: {} - {}",
                spinner[spinner_idx.0 % spinner.len()],
                batch.filename,
                batch
                    .games
                    .last()
                    .and_then(|g| g.date.as_ref())
                    .unwrap_or(&String::new()),
                res.status(),
                res.text().expect("decode response")
            );
        }
    });

    for arg in args.pgns {
        let file = File::open(&arg)?;

        let uncompressed: Box<dyn io::Read> = if arg.extension() == Some(OsStr::new("bz2")) {
            println!("Reading compressed {:?} ...", arg);
            Box::new(bzip2::read::MultiBzDecoder::new(file))
        } else {
            println!("Reading {:?} ...", arg);
            Box::new(file)
        };

        let mut reader = BufferedReader::new(uncompressed);

        let mut importer = Importer::new(tx.clone(), arg, args.batch_size);
        reader.read_all(&mut importer)?;
        importer.send();
    }

    drop(tx);
    bg.join().expect("bg join");
    Ok(())
}
