pub mod bz2_file;
use error_chain::error_chain;
use rayon::iter::*;
use regex::Regex;
use select::{document::Document, predicate::Name};
use url::{Position, Url};
use walkdir::{DirEntry, WalkDir};

use std::{
    collections::{HashSet, VecDeque},
    fs::{self, File},
    io::{self, stdin, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

const KB_SIZE: usize = 1024;
const MB_SIZE: usize = KB_SIZE * KB_SIZE;
const SEP_LEN: usize = 50;
const POST_MSG_REPLACE: usize = 70;
const REDIRECT_LINK: &str = "gflfastdlv2";

error_chain! {
    foreign_links {
        ReqError(reqwest::Error);
        IoError(std::io::Error);
        UrlParseError(url::ParseError);
    }
}

fn get_base_url(url: &Url, doc: &Document) -> Result<Url> {
    let base_tag_href = doc.find(Name("base")).filter_map(|n| n.attr("href")).nth(0);
    let base_url =
        base_tag_href.map_or_else(|| Url::parse(&url[..Position::BeforePath]), Url::parse)?;

    Ok(base_url)
}

/// Peform BFS on the `dl_url` that was provided
///
/// # Arguments
/// * `dl_url`      A &str which is the fastdl url
fn scrape_web(dl_url: &str) -> Result<Arc<RwLock<HashSet<String>>>> {
    // println!("{}{}\n", term_cursor::Goto(0, 1), "=".repeat(SEP_LEN));
    // println!("{}{}\n", term_cursor::Goto(0, 7), "=".repeat(SEP_LEN));

    // Store the links that will be downloaded
    let download_links = Arc::new(RwLock::new(HashSet::<String>::new()));
    // Stores the links that were visited
    let visited_paths = Arc::new(Mutex::new(HashSet::<String>::new()));
    // Stores the paths that were not visited
    let unvisited_paths = Mutex::new(VecDeque::<String>::new());

    // Parent directory of `dl_url`
    let parent_dir_url_1 = Url::parse(format!("{}{}", dl_url, "..").as_str())?
        .path()
        .to_string();
    // fastdl parent directory link results in no suffix "/" character
    // Use this to go from "/cstrike/" -> "/cstrike"
    let parent_dir_url_2 = {
        let mut temp_chars = parent_dir_url_1.chars();
        temp_chars.next_back();
        temp_chars.as_str().to_string()
    };

    // Visited links should include the parent directory and the `base_url`
    visited_paths.lock().unwrap().insert(String::from("/"));
    visited_paths.lock().unwrap().insert(parent_dir_url_1);
    visited_paths.lock().unwrap().insert(parent_dir_url_2);

    // Get the `base_url` of `dl_url`
    let temp_req = reqwest::blocking::get(dl_url)?.text()?;
    let temp_doc = Document::from(temp_req.as_str());
    let dl_url = Url::parse(dl_url)?;

    // Store the path we will first visit
    unvisited_paths
        .lock()
        .unwrap()
        .push_front(dl_url.path().to_string());

    // Iterate through every directory
    loop {
        // Base case: All paths/links have been visited
        if unvisited_paths.lock().unwrap().is_empty() {
            break;
        }

        // Thread handler which will join all threads (synchronize)
        let mut handler = Vec::new();
        // Iterate through every item
        //      Length is obtained because we don't want to deadlock
        //      and it's possible to get a runtime error
        let unvisited_len = unvisited_paths.lock().unwrap().len();

        // Iterate through every item in the directory
        for _ in 0..unvisited_len {
            // Pop the last visited object in the path
            let curr_path = unvisited_paths.lock().unwrap().pop_back().unwrap();
            // let curr_path = String::from(curr_path);

            // Move to the next path if the link was visited was already visited
            if visited_paths.lock().unwrap().contains(curr_path.as_str()) {
                continue;
            }

            // Clone the `visited_paths` and `download_links` for parallel storing of paths/links
            let visited_paths_clone = Arc::clone(&visited_paths);
            let download_links_clone = Arc::clone(&download_links);

            // Get the `base_url` of `dl_url`
            let base_url = get_base_url(&dl_url, &temp_doc)?;
            // `head` is used to perform HEADER req
            let head = reqwest::blocking::Client::builder()
                .timeout(None)
                .build()
                .unwrap();

            // Create a thread for each path (file/dir) to visit
            let t = std::thread::spawn(move || {
                // Used to join the threads together to prevent race conditions with the function terminating too early
                let new_paths = Arc::new(Mutex::new(VecDeque::new()));

                // fastdl parent directory link results in no suffix "/" character
                // Adding the `curr_path` without the suffix "/" is the same reasoning as above
                let curr_path_alt = {
                    let mut temp_chars = curr_path.chars();
                    temp_chars.next_back();
                    temp_chars.as_str().to_string()
                };

                // Add `curr_path` as a visited link
                visited_paths_clone
                    .lock()
                    .unwrap()
                    .insert(curr_path.clone());
                visited_paths_clone.lock().unwrap().insert(curr_path_alt);

                println!(
                    "{}Visited Paths:\t\t{}",
                    term_cursor::Goto(0, 3),
                    visited_paths_clone.lock().unwrap().len()
                );

                // Create a url out of the `dl_url` &str
                let url = base_url.join(curr_path.as_str()).unwrap();

                // GET Request containing all the links to recursively traverse
                let req = reqwest::blocking::get(url.as_str())
                    .unwrap()
                    .text()
                    .unwrap();

                // Iterate through the list of websites in `url`, parsing only the links (dir/files)
                let curr_path_links = Document::from(req.as_str())
                    .find(Name("a"))
                    .filter_map(|n| n.attr("href"))
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>();

                let curr_path_links_clone = curr_path_links.clone();
                let new_paths_clone = Arc::clone(&new_paths);

                // Iterate through all the url links and add the list to a checkable path if it was not seen
                // If the url link is a downloadable link, the url link will be added to `download_links`
                curr_path_links_clone.par_iter().for_each(|x| {
                    // Send HEADER requests (faster than GET) and parse in the format:
                    // {scheme}://{domain}/{path}
                    // Note: `path` includes a prepended / in the assignment of`next_site`
                    let new_url = url.join(x).unwrap();
                    let header = head.post(new_url).send().unwrap();
                    let scheme = header.url().scheme();
                    let domain = header.url().host_str().unwrap();
                    let path = header.url().path();
                    let next_site = format!("{scheme}://{domain}{path}");

                    // Append the paths we have not visited
                    // Conditions:
                    //  1. Set contains a visited path
                    //  2. String contains "index.html"
                    //  3. String contains ".tmp"
                    //  4. String contains ".ztmp"
                    if !visited_paths_clone.lock().unwrap().contains(path)
                        && !path.contains("index.html")
                        && !path.contains(".tmp")
                        && !path.contains(".ztmp")
                    {
                        if !path.contains(REDIRECT_LINK) && !path.contains("maps/") {
                            // Do not add "fastdlv2" links - We don't want to recurse through fastdlv2
                            new_paths_clone.lock().unwrap().push_front(path.to_string());
                        } else if (path.contains(REDIRECT_LINK)
                            && !path.ends_with("/")
                            && !path.contains("maps/"))
                            || (path.contains("maps/") && path.contains("ze_"))
                        {
                            // Only add "fastdlv2" in our `download_links` Vec
                            // Second case ensures that the fastdlv2 directories are not being recursed as well
                            // I'm not sure why there are fastdlv2 directory links
                            print!(
                                "{}{}{}",
                                term_cursor::Goto(0, 5),
                                next_site,
                                " ".repeat(POST_MSG_REPLACE)
                            );

                            download_links_clone.write().unwrap().insert(next_site);

                            println!(
                                "{}Downloadable Links:\t{}",
                                term_cursor::Goto(0, 4),
                                download_links_clone.write().unwrap().len()
                            );
                        }
                    }
                });

                // Each thread will return a Vec of all the links to its directory
                return new_paths;
            });

            // Append all threads that are traversing the directory
            handler.push(t);
        }

        // Join all threads, then append all the vectors into the vectors (thread-safety)
        for t in handler {
            let mut unvisited_vec_thread = t.join().unwrap().lock().unwrap().to_owned();

            // Append new links to the unvisited path
            unvisited_paths
                .lock()
                .unwrap()
                .append(&mut unvisited_vec_thread);
        }
    }

    // Clear the list of files/paths that were checked
    println!("{}{}", term_cursor::Goto(0, 5), " ".repeat(170));
    // println!("{}", term_cursor::Goto(0, 8));

    Ok(download_links)
}

/// Downloads all the files in `dl_links`
/// Create directories inside of the current directory for the path of the file if it does not exist
///
/// # Arguments
/// `dl_links`      HashSet that contains all the download links that will be downloaded and stored
fn download_files(dl_links: &Arc<RwLock<HashSet<String>>>) {
    let idx = Mutex::new(0);
    let curr_path = std::env::current_dir().unwrap();

    // Use regex to obtain the directory path and file name
    let dl_url_paths = |dl_url: &str| -> (PathBuf, PathBuf) {
        let re = Regex::new("(.+?)//(.+?)/(.*+)/(.*+)").unwrap();
        let captures = re.captures(dl_url).unwrap();

        let dir = &captures[3].replace("/", "\\");
        let file = &captures[4];

        let dir_path_str = format!("{}\\{}", curr_path.to_str().unwrap(), dir);
        let dir_path = Path::new(dir_path_str.as_str());
        let file_path_str = format!("{}\\{}", dir_path_str, file);
        let file_path = Path::new(file_path_str.as_str());

        (dir_path.to_path_buf(), file_path.to_path_buf())
    };

    // Iterate and get all the paths that are visited
    dl_links.read().unwrap().par_iter().for_each(|dl_url| {
        // Get PathBufs of the file and its directory
        let (dir_path, file_path) = dl_url_paths(dl_url);

        // Track our item status and info (You can disable and it may improve runtime)
        *idx.lock().unwrap() += 1;

        print!(
            "
{}[ {} / {} ]
{}Link:\t\t\t{}{}
{}File:\t\t\t{}{}
{}Dir:\t\t\t{}{}",
            // Total Left Params
            term_cursor::Goto(0, 10),
            idx.lock().unwrap(),
            dl_links.read().unwrap().len(),
            // Link Params
            term_cursor::Goto(0, 11),
            dl_url,
            " ".repeat(POST_MSG_REPLACE),
            // Capture Params
            term_cursor::Goto(0, 12),
            file_path.to_str().unwrap(),
            " ".repeat(POST_MSG_REPLACE),
            // Dir Params
            term_cursor::Goto(0, 13),
            dir_path.to_str().unwrap(),
            " ".repeat(POST_MSG_REPLACE),
        );

        // Recursively create directories to the folders we want to search
        std::fs::create_dir_all(dir_path).unwrap();

        // Get request the file link and store it in the directory path
        loop {
            // If the request times out, send another request
            if let Ok(response) = reqwest::blocking::get(dl_url) {
                if let Ok(file_bytes) = response.bytes() {
                    File::create(file_path)
                        .unwrap()
                        .write_all(&file_bytes)
                        .unwrap();
                    break;
                }
            }

            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// Decodes all bz2 files in the current directory by recursively searching through all the paths
/// After all paths are decoded, the original bz2 files are deleted
fn decode_files(corrupt_files: &Mutex<HashSet<String>>) {
    // Recursively collect files ending with .bz2
    let dirs = WalkDir::new(".")
        .into_iter()
        .flatten()
        .filter(|dir| dir.file_name().to_str().unwrap().trim().ends_with(".bz2"))
        .collect::<Vec<DirEntry>>();

    let cmp_dir_size = Mutex::<usize>::new(0);

    // Print all the bz2 files that will be decoded
    // dirs.par_iter()
    // .for_each(|f| println!("{}", f.file_name().to_str().unwrap().trim()));

    // File print separator
    // println!("\n{}\n{}\n", "=".repeat(SEP_LEN), "=".repeat(SEP_LEN));

    // Iterate through every file and decode it
    dirs.par_iter().for_each(|dir| {
        // Grab the {bz2/bsp} file name and path
        let file_name = dir
            .file_name()
            .to_str()
            .expect("Failed to convert &OSStr to &str");
        let file_name_path = dir.path().to_str().unwrap();

        let output_name_path = file_name_path.replace(".bz2", "");

        // Open the file and check if it's a bz2 file
        if let Ok(f) = File::open(dir.path()) {
            // Create the decoder (converts bz2 to bsp)
            let mut decoder = bz2_file::BZ2File::new(f);

            match decoder.decode_block() {
                Ok(_) => {}
                _ => {
                    corrupt_files.lock().unwrap().insert(file_name.to_string());
                    return;
                }
            }

            // Increment the compared value (for status checking)
            *cmp_dir_size.lock().unwrap() += 1;

            // Print the file information
            print!(
                "
                {}File:\t\t\t{}{}
                {}Directory:\t\t{}{}
                {}Size:\t\t\t{} MB{}
                {}Finished Decoding:\t{} / {}{}
                ",
                // File Params
                term_cursor::Goto(0, 18),
                file_name,
                " ".repeat(POST_MSG_REPLACE),
                // Directory Params
                term_cursor::Goto(0, 19),
                file_name_path.replace(file_name, ""),
                " ".repeat(POST_MSG_REPLACE),
                // Size Params
                term_cursor::Goto(0, 20),
                decoder.decoded_block.get_mut().len() as f32 / MB_SIZE as f32,
                " ".repeat(POST_MSG_REPLACE),
                // Finished Decoding Params
                term_cursor::Goto(0, 21),
                cmp_dir_size.lock().unwrap(),
                dirs.len(),
                " ".repeat(POST_MSG_REPLACE),
            );

            // Decoding completion separator
            // println!("{}{}\n", "=".repeat(SEP_LEN));

            // Create the bsp file
            let mut output = File::create(output_name_path).unwrap();

            if let Err(_) = output.write_all(&decoder.decoded_block.get_mut()) {
                corrupt_files
                    .lock()
                    .unwrap()
                    .insert(format!("{}", file_name_path.to_string(),));
            }

            // Delete the bz2 file
            fs::remove_file(file_name_path).unwrap();
        }
    });
}

fn print_console_gui() {
    print!("{}", term_cursor::Clear);

    print!(
        "
        {}Searching All Paths
        {}{}
        {}Downloading Files
        {}{}
        {}Decoding All Files (bz2 -> original file)
        {}{}
        ",
        // Checking All Paths
        term_cursor::Goto((SEP_LEN / 8) as i32, 0),
        term_cursor::Goto(0, 1),
        "=".repeat(SEP_LEN),
        // Downloading Files
        term_cursor::Goto((SEP_LEN / 8) as i32, 7),
        term_cursor::Goto(0, 8),
        "=".repeat(SEP_LEN),
        // Decoding All Files
        term_cursor::Goto((SEP_LEN / 8) as i32, 15),
        term_cursor::Goto(0, 16),
        "=".repeat(SEP_LEN),
    );
}

fn main() -> Result<()> {
    // TIMER START
    let timer = Instant::now();
    let corrupt_files = Mutex::new(HashSet::<String>::new());

    // Prints a real-time readable console output
    print_console_gui();

    // TODO: Add support for ze_* maps
    // CS:S
    let mut fastdl_urls = Vec::<&str>::new();
    // let fastdl_urls = Vec::with_capacity(5);
    // fastdl_urls.push("https://fastdl.gflclan.com/cstrike/maps/");
    // fastdl_urls.push("https://fastdl.gflclan.com/cstrike/materials/");
    // fastdl_urls.push("https://fastdl.gflclan.com/cstrike/models/");
    // fastdl_urls.push("https://fastdl.gflclan.com/cstrike/resource/");
    fastdl_urls.push("https://fastdl.gflclan.com/cstrike/sound/");
    // fastdl_urls.push("https://fastdl.gflclan.com/cstrike/");

    for url in fastdl_urls.to_owned() {
        let dl_links = scrape_web(url).unwrap();

        // Create directories for the files, then download and store them in their respective directories
        download_files(&dl_links);

        // Grabs all the bz2 files and decodes them, making bsp files
        // Then, the bz2 files are deleted, keeping only the bsp files
        decode_files(&corrupt_files);
    }

    println!(
        "{}{}
        {}URL:\t{:#?}
        {}Time:\t{}
        {}{}",
        // Separator Params
        term_cursor::Goto(0, 23),
        "=".repeat(25),
        // URL
        term_cursor::Goto(0, 24),
        fastdl_urls,
        // Time
        term_cursor::Goto(0, 25),
        timer.elapsed().as_secs_f32(),
        // Separator
        term_cursor::Goto(0, 26),
        "=".repeat(25)
    );

    print!(
        "{}Files that failed to decompress correctly: {:#?}{}",
        term_cursor::Goto(0, 28),
        corrupt_files.lock().unwrap(),
        term_cursor::Goto(0, 35),
    );

    // User Input to confirm that all maps are downloaded/extracted
    print!("{}Press Enter to exit...", term_cursor::Goto(0, 42));
    Write::flush(&mut io::stdout()).expect("Failed to flush the ");

    stdin().read(&mut [0]).unwrap();

    // for corr_f in corrupt_files.lock().unwrap().iter() {
    // println!("{}", corr_f);
    // }
    // TIMER END

    Ok(())
}
