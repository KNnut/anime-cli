use std::fs;
use std::path::{Path, PathBuf};
use std::ffi::OsStr;
use std::process::exit;
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::io;
use getopts::Options;
use pbr::{MultiBar, Pipe, ProgressBar, Units};

mod anime_dl;
mod anime_find;

static IRC_SERVER: &str = "irc.rizon.net:6667";
static IRC_CHANNEL: &str = "nibl";
static IRC_NICKNAME: &str = "randomRustacean";


const AUDIO_EXTENSIONS: &'static [&'static str] = &["aif", "cda", "mid", "midi", "mp3",
                                                    "mpa", "ogg", "wav", "wma", "wpl"];

const VIDEO_EXTENSIONS: &'static [&'static str] = &["3g2", "3gp", "avi", "flv", "h264",
                                                    "m4v", "mkv", "mov", "mp4", "mpg",
                                                    "mpeg", "rm", "swf", "vob", "wmv"];

fn print_usage(program: &str, opts: Options) {
    let msg = opts.short_usage(&program);
    print!("{}", opts.usage(&msg));
    println!("\n\
    ===================================\n\
    Helpful Tips:                      \n\
    Try to keep your anime name simple \n\
    and use quotes when you use -q     \n\
    e.g. \"sakamoto\"                  \n\
                                       \n\
    Common resolutions 480/720/1080    \n\
                                       \n\
    Batch end number means last episode\n\
    in a range of episodes             \n\
      e.g. episode ------------> batch \n\
      everything from 1 -------> 10    \n\
                                       \n\
    You can apply default resolution   \n\
    and default batch # with a blank   \n\
    ===================================\n
    ");
}

fn get_cli_input(prompt: &str) -> String {
    println!("{}", prompt);
    let mut input = String::new();
    if let Err(e) = io::stdin().read_line(&mut input) {
        eprintln!("{}", e);
        eprintln!("Please enter a normal query");
        exit(1);
    }
    input.to_string().replace(|c: char| c == '\n' || c == '\r', "")
}

fn main() {
    let args: Vec<String> = std::env::args().collect(); // We collect args here

    let mut query: String;
    let resolution: Option<u16>;
    let mut episode: Option<u16> = None;
    let mut last_ep: Option<u16> = None;
    let play: bool;

    // Are we in cli mode or prompt mode?
    if args.len() > 1 {
        let program = args[0].clone();
        let mut opts = Options::new();
        opts.optopt("q", "query", "Query to run", "QUERY")
            .optopt("e", "episode", "Start from this episode", "NUMBER")
            .optopt("t", "to", "Last episode", "NUMBER")
            .optopt("r", "resolution", "Resolution", "NUMBER")
            .optflag("p", "play", "Open with a player")
            .optflag("h", "help", "print this help menu");
    
        let matches = match opts.parse(&args[1..]) {
            Ok(m) => m,
            Err(error) => {
                eprintln!("{}.", error);
                eprintln!("{}", opts.short_usage(&program));
                exit(1);
            }
        };
    
        // Unfortunately, cannot use getopts to check for a single optional flag
        // https://github.com/rust-lang-nursery/getopts/issues/46
        if matches.opt_present("h") {
            print_usage(&program, opts);
            return
        }

        play = matches.opt_present("p");

        resolution = match matches.opt_str("r").as_ref().map(String::as_str) {
            Some("0") => None,
            Some(r) => Some(parse_number(String::from(r))),
            _ => Some(720),
        };

        query = matches.opt_str("q").unwrap();

        if let Some(ep) = matches.opt_str("e") {
            episode = Some(parse_number(ep))
        }

        if let Some(t) = matches.opt_str("t") {
            last_ep = Some(parse_number(t))
        }
    } else {
        println!("Welcome to anime-cli");
        println!("Default: resolution => None | episode => None | to == episode | play => false");
        println!("Resolution shortcut: 1 => 480p | 2 => 720p | 3 => 1080p");
        query = get_cli_input("Anime/Movie name: ");
        resolution =  match parse_number(get_cli_input("Resolution: ")) {
            0 => None,
            1 => Some(480),
            2 => Some(720),
            3 => Some(1080),
            r => Some(r),
        };
        episode = match parse_number(get_cli_input("Start from the episode: ")) {
            0 => None,
            e => Some(e),
        };
        last_ep = match parse_number(get_cli_input("To this episode: ")) {
            0 => { if episode.is_some() { episode } else { None } },
            b => Some(b),
        };
        play = get_cli_input("Play now? [y/N]: ").to_ascii_lowercase().eq("y");
    }

    // If resolution entered, add a resolution to the query
    if let Some(res) = resolution {
        query.push(' ');
        query.push_str(&res.to_string());
    }

    if last_ep.is_some() && last_ep.unwrap() < episode.unwrap_or(1) { // Make sure batch end is never smaller than episode start
        last_ep = episode;
    }

    let mut dccpackages = vec![];

    let mut num_episodes = 0;  // Search for packs, verify it is media, and add to a list
    for i in episode.unwrap_or(1)..last_ep.unwrap_or(episode.unwrap_or(1)) + 1 {
        if episode.is_some() || last_ep.is_some() {
            println!("Searching for {} episode {}", query, i);
        } else {
            println!("Searching for {}", query);
        }
        match anime_find::find_package(&query, &episode.or(last_ep).and(Some(i))) {
            Ok(p) => {
                match Path::new(&p.filename).extension().and_then(OsStr::to_str) {
                    Some(ext) => {
                        if !AUDIO_EXTENSIONS.contains(&ext) && !VIDEO_EXTENSIONS.contains(&ext) {
                            eprintln!("Warning, this is not a media file! Skipping");
                        } else {
                            dccpackages.push(p);
                            num_episodes += 1;
                        }
                    },
                    _ => { eprintln!("Warning, this file has no extension, skipping"); }
                }
            },
            Err(e) => {
                eprintln!("{}", e);
            }
        };
    }

    if num_episodes == 0 { exit(1); }

    match fs::create_dir(&query) { // organize
        Ok(_) => println!{"Created folder {}", &query},
        _ => eprintln!{"Could not create a new folder, does it exist?"},
    };
    let dir_path = Path::new(&query).to_owned();

    let terminal_dimensions = term_size::dimensions();

    let mut channel_senders = vec![];
    let mut multi_bar = MultiBar::new();
    let mut multi_bar_handles = vec![];
    let (status_bar_sender, status_bar_receiver) = channel();

    let mut pb_message = String::new();
    for i in 0..dccpackages.len() { //create bars for all our downloads
        let (sender, receiver) = channel();
        let handle;

        match terminal_dimensions {
            Some((w, _)) if dccpackages[i].filename.len() > &w/2 => { // trim the filename
                let acceptable_length = w / 2;
                let first_half = &dccpackages[i].filename[..dccpackages[i].filename.char_indices().nth(acceptable_length/2).unwrap().0];
                let second_half = &dccpackages[i].filename[dccpackages[i].filename.char_indices().nth_back(acceptable_length/2).unwrap().0..];
                if acceptable_length < 50 {
                    pb_message.push_str(first_half);
                }
                pb_message.push_str("...");
                pb_message.push_str(second_half);
            },
            _ => pb_message.push_str(&dccpackages[i].filename)
        };
        pb_message.push_str(": ");

        let mut progress_bar = multi_bar.create_bar(dccpackages[i].sizekbits as u64);
        progress_bar.set_units(Units::Bytes);
        progress_bar.message(&pb_message);
        pb_message.clear();

        handle = thread::spawn(move || { // create an individual thread for each bar in the multibar with its own i/o
            update_bar(&mut progress_bar, receiver);
        });

        channel_senders.push(sender);
        multi_bar_handles.push(handle);
    }

    let mut status_bar = multi_bar.create_bar(dccpackages.len() as u64);
    status_bar.set_units(Units::Default);
    status_bar.message(&format!("{}: ", "Waiting..."));
    let status_bar_handle = thread::spawn(move || {
        update_status_bar(&mut status_bar, status_bar_receiver);
    });
    multi_bar_handles.push(status_bar_handle);

    let _ = thread::spawn(move || { // multi bar listen is blocking
        multi_bar.listen();
    });

    let irc_request = anime_dl::IRCRequest {
        server: IRC_SERVER.to_string(),
        channel: IRC_CHANNEL.to_string(),
        nickname: IRC_NICKNAME.to_string(),
        bot: dccpackages.clone().into_iter().map(|package| package.bot).collect(),
        packages: dccpackages.clone().into_iter().map(|package| package.number.to_string()).collect(),
    };

     //If we don't have mpv, we'll open the file using default media app. We can't really hook into it so we limit to 1 file so no spam
    let video_handle = if play && (num_episodes == 1 || cfg!(feature = "mpv")) {
        Some(play_video(dccpackages.into_iter().map(|package| package.filename).collect(), dir_path.clone()))
    } else {
        None
    };

    if let Err(e) = anime_dl::connect_and_download(irc_request, channel_senders, status_bar_sender, dir_path.clone()) {
        eprintln!("{}", e);
        exit(1);
    };
    if let Some(vh) = video_handle {
        vh.join().unwrap();
    }
    multi_bar_handles.into_iter().for_each(|handle| handle.join().unwrap());
}

fn update_status_bar(progress_bar: &mut ProgressBar<Pipe>, receiver: Receiver<String>) {
    progress_bar.tick();
    while let Ok(progress) = receiver.recv() {
        progress_bar.message(&format!("{} ", progress));
        progress_bar.tick();
        match progress.as_str() {
            "Episode Finished Downloading" => { progress_bar.inc(); },
            "Success" => return progress_bar.finish(),
            _ => {}
        }
    }
}

fn update_bar(progress_bar: &mut ProgressBar<Pipe>, receiver: Receiver<i64>) {
    progress_bar.tick();
    while let Ok(progress) = receiver.recv() {
        if progress > 0 {
            progress_bar.set(progress as u64);
        } else {
            return progress_bar.finish();
        }
    };
}

fn parse_number(str_num: String) -> u16 {
    let c_str_num = str_num.replace(|c: char| !c.is_numeric(), "");
    match c_str_num.parse::<u16>() {
        Ok(e) => e,
        Err(err) => {
            if err.to_string() == "cannot parse integer from empty string" {
                0
            } else {
                eprintln!("Input must be numeric.");
                exit(1);
            }
        }
    }
}

#[cfg(feature = "mpv")]
fn play_video(filenames: Vec<String>, dir_path: PathBuf) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_secs(5));
        let mut i = 0;
        let mut timeout = 0;
        let mut filename = &filenames[i];
        let video_path = dir_path.join(filename);
        while timeout < 5 { //Initial connection waiting
            if !video_path.is_file() {
                timeout += 1;
                thread::sleep(std::time::Duration::from_secs(5));
            } else {
                break;
            }
        }
        let mut mpv_builder = mpv::MpvHandlerBuilder::new().expect("Failed to init MPV builder");
        if video_path.is_file() {
            let video_path = video_path
                .to_str()
                .expect("Expected a string for Path, got None");
            mpv_builder.set_option("osc", true).unwrap();
            mpv_builder
                .set_option("input-default-bindings", true)
                .unwrap();
            mpv_builder.set_option("input-vo-keyboard", true).unwrap();
            let mut mpv = mpv_builder.build().expect("Failed to build MPV handler");
            mpv.command(&["loadfile", video_path as &str])
                .expect("Error loading file");
            'main: loop {
                while let Some(event) = mpv.wait_event(0.0) {
                    //println!("{:?}", event);
                    match event {
                        mpv::Event::Shutdown => {
                            break 'main;
                        }
                        mpv::Event::Idle => {
                            if i >= filenames.len() {
                                break 'main;
                            }
                        }
                        mpv::Event::EndFile(Ok(mpv::EndFileReason::MPV_END_FILE_REASON_EOF)) => {
                            i += 1;
                            if i >= filenames.len() {
                                break 'main;
                            }
                            filename = &filenames[i];
                            let next_video_path = dir_path.join(filename);
                            if next_video_path.is_file() {
                                let next_video_path = next_video_path
                                    .to_str()
                                    .expect("Expected a string for Path, got None");
                                mpv.command(&["loadfile", next_video_path as &str])
                                    .expect("Error loading file");
                            } else {
                                eprintln!(
                                    "A file is required; {} is not a valid file",
                                    next_video_path.to_str().unwrap()
                                );
                            }
                        }
                        _ => {}
                    };
                }
            }
        } else {
            eprintln!(
                "A file is required; {} is not a valid file",
                video_path.to_str().unwrap()
            );
        }
    })
}

#[cfg(not(feature = "mpv"))]
fn play_video(filenames: Vec<String>, dir_path: PathBuf) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_secs(5));
        let filename = &filenames[0];
        let video_path = dir_path.join(filename);

        let mut timeout = 0;
        while timeout < 5 { //Initial connection waiting
            if !video_path.is_file() {
                timeout += 1;
                thread::sleep(std::time::Duration::from_secs(5));
            } else {
                break;
            }
        }
        if let Err(e) = opener::open(video_path) {
            eprintln!("{:?}", e);
        };
    })
}
