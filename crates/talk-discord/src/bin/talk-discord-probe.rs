//! Developer probe for Discord voice, run by hand with a test account.
//! It is not part of Talk. See docs/discord.md#testing.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, Cursor, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use songbird::input::RawAdapter;
use talk_discord::{
    api::Api,
    gateway::Gateway,
    ready, token,
    voice::{CHANNELS, Call, SAMPLE_RATE},
};

const USAGE: &str = "usage: talk-discord-probe <command>
  store-token              save the test account's token in the OS credential store
  whoami                   show which account the saved token belongs to
  servers                  list servers and their voice channels
  join <channel> [seconds] join a voice channel, play beeps, record what others say
  forget-token             remove the saved token";
const RECORDINGS: &str = "target/discord-probe";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("store-token") => {
            let token = read_hidden("Paste the test account's token (hidden): ")?;
            token::store(&token)?;
            println!("Saved in the OS credential store.");
            whoami().await
        }
        Some("whoami") => whoami().await,
        Some("servers") => servers().await,
        Some("join") => {
            let channel = args.get(1).context(USAGE)?;
            let seconds = match args.get(2) {
                Some(seconds) => seconds.parse().context("seconds must be a number")?,
                None => 60,
            };
            join(channel, seconds).await
        }
        Some("forget-token") => {
            token::forget()?;
            println!("Removed the token from the OS credential store.");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

async fn whoami() -> Result<()> {
    let user = Api::new(token::load()?)?.me().await?;
    let name = user.global_name.as_deref().unwrap_or(&user.username);
    println!(
        "Signed in to Discord as {name} (@{}), id {}.",
        user.username, user.id
    );
    Ok(())
}

async fn servers() -> Result<()> {
    let (gateway, ready) = Gateway::connect(&token::load()?).await?;
    gateway.close().await;
    for server in ready::servers(&ready) {
        println!("{} ({})", server.name, server.id);
        for channel in &server.voice_channels {
            println!("  voice: {} ({})", channel.name, channel.id);
        }
    }
    Ok(())
}

async fn join(channel_id: &str, seconds: u64) -> Result<()> {
    let (mut gateway, ready) = Gateway::connect(&token::load()?).await?;
    let me = ready::user_id(&ready)
        .context("READY has no user")?
        .to_owned();
    let (server, channel) = ready::servers(&ready)
        .into_iter()
        .find_map(|server| {
            let channel = server.voice_channels.iter().find(|c| c.id == channel_id)?;
            let name = channel.name.clone();
            Some((server, name))
        })
        .context("no voice channel with that id; run servers to list them")?;
    println!("Joining {channel} in {}...", server.name);

    let mut call = Call::join(&mut gateway, &me, &server.id, channel_id).await?;
    call.play(RawAdapter::new(Cursor::new(beeps(seconds)), SAMPLE_RATE, CHANNELS.into()).into());
    println!(
        "Playing a beep every second for {seconds} s. Speak in the channel; Ctrl+C ends early."
    );

    // Keep the gateway alive and show who joins or leaves meanwhile.
    let mut names = HashMap::new();
    let end = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(end);
    loop {
        tokio::select! {
            () = &mut end => break,
            _ = tokio::signal::ctrl_c() => break,
            event = gateway.next() => {
                let event = event?;
                if event.name != "VOICE_STATE_UPDATE" || event.data["user_id"] == me.as_str() {
                    continue;
                }
                let user = event.data["user_id"].as_str().unwrap_or_default().to_owned();
                let name = event.data["member"]["user"]["username"]
                    .as_str()
                    .unwrap_or(&user)
                    .to_owned();
                names.insert(user, name.clone());
                if event.data["channel_id"] == channel_id {
                    println!("{name} is in the channel.");
                } else {
                    println!("{name} left the channel.");
                }
            }
        }
    }

    call.leave();
    gateway.update_voice_state(Some(&server.id), None).await?;
    gateway.close().await;
    save_recordings(&call, &names)
}

/// Write each speaker's audio to a WAV file and summarise it.
fn save_recordings(call: &Call, names: &HashMap<String, String>) -> Result<()> {
    let audio = call.received.audio.lock().unwrap();
    let speakers = call.received.speakers.lock().unwrap();
    if audio.is_empty() {
        println!("Received no audio.");
        return Ok(());
    }
    fs::create_dir_all(RECORDINGS)?;
    for (ssrc, samples) in audio.iter() {
        let user = speakers.get(ssrc).map(u64::to_string);
        let label = user
            .as_ref()
            .map(|user| names.get(user).cloned().unwrap_or_else(|| user.clone()))
            .unwrap_or_else(|| format!("source-{ssrc}"));
        let path = Path::new(RECORDINGS).join(format!("{label}.wav"));
        write_wav(&path, samples)?;
        let seconds = samples.len() as f64 / f64::from(SAMPLE_RATE) / f64::from(CHANNELS);
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        let peak_db = 20.0 * (f64::from(peak.max(1)) / 32768.0).log10();
        println!(
            "Received {seconds:.1} s from {label}, peak {peak_db:.1} dBFS: {}",
            path.display()
        );
    }
    Ok(())
}

/// A short 660 Hz beep at the start of every second, as interleaved stereo
/// little-endian f32, the format songbird's raw adapter reads.
fn beeps(seconds: u64) -> Vec<u8> {
    let rate = SAMPLE_RATE as usize;
    let frames = rate * seconds as usize;
    let beep = rate * 3 / 10;
    let fade = rate / 200;
    let mut bytes = Vec::with_capacity(frames * usize::from(CHANNELS) * 4);
    for frame in 0..frames {
        let t = frame % rate;
        let sample = if t < beep {
            let envelope = (t.min(beep - t) as f32 / fade as f32).min(1.0);
            let phase = 2.0 * std::f32::consts::PI * 660.0 * frame as f32 / rate as f32;
            0.25 * envelope * phase.sin()
        } else {
            0.0
        };
        for _ in 0..CHANNELS {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    bytes
}

fn write_wav(path: &Path, samples: &[i16]) -> Result<()> {
    let data_len = u32::try_from(samples.len() * 2)?;
    let block_align = CHANNELS * 2;
    let mut file = File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&CHANNELS.to_le_bytes())?;
    file.write_all(&SAMPLE_RATE.to_le_bytes())?;
    file.write_all(&(SAMPLE_RATE * u32::from(block_align)).to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

/// Read one line from the terminal without echoing it.
fn read_hidden(prompt: &str) -> Result<String> {
    let tty = || File::options().read(true).write(true).open("/dev/tty");
    let stty = |arg: &str| -> Result<()> {
        let status = Command::new("stty")
            .arg(arg)
            .stdin(tty().context("needs a terminal")?)
            .stdout(Stdio::null())
            .status()
            .context("could not run stty")?;
        if !status.success() {
            bail!("could not hide terminal input");
        }
        Ok(())
    };
    let mut terminal = tty().context("needs a terminal")?;
    write!(terminal, "{prompt}")?;
    terminal.flush()?;
    stty("-echo")?;
    let mut line = String::new();
    let read = std::io::BufReader::new(tty()?).read_line(&mut line);
    stty("echo")?;
    writeln!(terminal)?;
    read.context("could not read the token")?;
    Ok(line)
}
