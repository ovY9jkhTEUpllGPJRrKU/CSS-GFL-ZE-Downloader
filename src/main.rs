pub mod bz2_file;
use crossbeam::channel;
use error_chain::error_chain;
use rayon::{iter::*, Scope};
use select::{document::Document, predicate::Name};
use url::{Position, Url};
use walkdir::{DirEntry, WalkDir};

use std::{
    collections::{HashSet, VecDeque},
    fs::{self, File},
    io::Write,
    sync::{mpsc, Arc, Mutex},
    time::Instant,
};

const KB_SIZE: usize = 1024;
const MB_SIZE: usize = KB_SIZE * KB_SIZE;
const SEP_LEN: usize = 50;

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
fn scrape_web<'a>(dl_url: &str) -> Result<()> {
    // let (tx, rx) = channel::unbounded();

    // Store the links that will be downloaded
    let download_links = Arc::new(Mutex::new(HashSet::<String>::new()));
    // Stores the links that were visited
    let visited_paths = Arc::new(Mutex::new(HashSet::<String>::new()));
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
        for idx in 0..unvisited_len {
            let curr_path = unvisited_paths.lock().unwrap().pop_back().unwrap();
            println!("[{idx}] {curr_path}");

            // Move to the next path if it was already visited
            if visited_paths.lock().unwrap().contains(curr_path.as_str()) {
                unvisited_paths.lock().unwrap().pop_back();

                continue;
            }

            let visited_paths_clone = Arc::clone(&visited_paths);
            let download_links_clone = Arc::clone(&download_links);

            // Get the `base_url` of `dl_url`
            let base_url = get_base_url(&dl_url, &temp_doc)?;
            // `head` is used to perform HEADER req
            let head = reqwest::blocking::Client::new();

            let t = std::thread::spawn(move || {
                let mut new_paths = VecDeque::new();

                // fastdl parent directory link results in no suffix "/" character
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

                // Create a url out of the `dl_url` &str
                let url = base_url.join(curr_path.as_str()).unwrap();

                // GET Request containing all the links to recursively traverse
                let req = reqwest::blocking::get(url.as_str())
                    .unwrap()
                    .text()
                    .unwrap();

                // Iterate through the list of websites in `url`, parsing only the links (dir/files)
                Document::from(req.as_str())
                    .find(Name("a"))
                    .filter_map(|n| n.attr("href"))
                    .for_each(|x| {
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
                            // DEBUG: Print the header information
                            // println!("{}", "=".repeat(SEP_LEN));
                            // println!("\nDomain: {}", domain);
                            // println!("Path: {}", path);
                            // println!("Download Link: {}\n", next_site);
                            // println!("{}", "=".repeat(SEP_LEN));

                            if !path.contains("gflfastdlv2") {
                                // Do not add "fastdlv2" links - We don't want to recurse through fastdlv2
                                new_paths.push_front(path.to_string());
                            } else {
                                // Only add "fastdlv2" in our `download_links` Vec
                                download_links_clone.lock().unwrap().insert(next_site);
                            }
                        }
                    });

                // DEBUG: Print out the status of every iteration of the loop
                println!("{}\n", "=".repeat(SEP_LEN));
                println!("Status:");
                println!(
                    "Visited Paths:\t\t{}",
                    visited_paths_clone.lock().unwrap().len()
                );
                println!(
                    "Downloadable Links:\t{}\n",
                    download_links_clone.lock().unwrap().len()
                );
                println!("{}\n", "=".repeat(SEP_LEN));

                return new_paths;
            });

            // Append all threads that are traversing the directory
            handler.push(t);
        }

        // Join all all threads, then append all the vectors into the vectors
        for t in handler.into_iter() {
            let mut unvisited_vec_thread = t.join().unwrap();

            unvisited_paths
                .lock()
                .unwrap()
                .append(&mut unvisited_vec_thread);
        }
    }

    Ok(())
}

fn decode_files() {
    // Recursively collect files ending with .bz2
    let dirs = WalkDir::new(".")
        .into_iter()
        .flatten()
        .filter(|dir| dir.file_name().to_str().unwrap().trim().ends_with(".bz2"))
        .collect::<Vec<DirEntry>>();

    let cmp_dir_size = Mutex::<usize>::new(0);

    // Print all the bz2 files that will be decoded
    dirs.par_iter()
        .for_each(|f| println!("{}", f.file_name().to_str().unwrap().trim()));

    // File print separator
    println!("\n{}\n{}\n", "=".repeat(SEP_LEN), "=".repeat(SEP_LEN));

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
            decoder.decode_block();

            // Increment the compared value (for status checking)
            *cmp_dir_size.lock().unwrap() += 1;

            // Print the file information
            println!(
                "File: {}\nDirectory: {}\nSize: {} MB\n",
                file_name,
                file_name_path.replace(file_name, ""),
                decoder.decoded_block.get_mut().len() as f32 / MB_SIZE as f32
            );

            // Print the completed decoding amount
            println!(
                "Finished Decoding:\t{} / {} Files",
                cmp_dir_size.lock().unwrap(),
                dirs.len()
            );

            // Decoding completion separator
            println!("{}\n", "=".repeat(SEP_LEN));

            // Create the bsp file
            // let mut output = File::create(output_name_path).unwrap();
            // output.write_all(&decoder.decoded_block.get_mut()).unwrap();

            // Delete the bz2 file
            // fs::remove_file(file_name_path).unwrap();
        }
    });
}

fn main() -> Result<()> {
    // TIMER START
    let timer = Instant::now();

    // Parse through the Fastdl site
    scrape_web(r"https://fastdl.gflclan.com/cstrike/materials/").unwrap();
    // scrape_web(r"https://fastdl.gflclan.com/cstrike/").unwrap();
    // r"https://fastdl.gflclan.com/cstrike/";

    // Grabs all the bz2 files and decodes them, making bsp files
    // Then, the bz2 files are deleted, keeping only the bsp files
    // decode_files();

    println!("\n{}", "=".repeat(25));
    println!("Time: {}", timer.elapsed().as_secs_f32());
    println!("{}\n", "=".repeat(25));
    // TIMER END

    Ok(())
}
