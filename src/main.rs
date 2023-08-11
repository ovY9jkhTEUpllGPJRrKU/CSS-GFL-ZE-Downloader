pub mod bz2_file;
use error_chain::error_chain;
use rayon::iter::*;

use select::{document::Document, predicate::Name};
use std::{
    collections::{HashSet, VecDeque},
    fs::{self, File},
    io::Write,
    sync::Mutex,
    time::Instant,
};
use url::{Position, Url};
use walkdir::{DirEntry, WalkDir};

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
fn scrape_web(dl_url: &str) -> Result<()> {
    // Store the links that will be downloaded
    let mut download_links = Vec::<String>::new();
    // Stores the links that were visited
    let mut visited_paths = HashSet::<String>::new();
    let mut unvisited_paths = VecDeque::<String>::new();

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

    // Get the `base_url` of `dl_url`
    let temp_req = reqwest::blocking::get(dl_url)?.text()?;
    let temp_doc = Document::from(temp_req.as_str());
    let dl_url = Url::parse(dl_url)?;
    let base_url = get_base_url(&dl_url, &temp_doc)?;

    // Visited links should include the parent directory and the `base_url`
    visited_paths.insert(String::from("/"));
    visited_paths.insert(parent_dir_url_1);
    visited_paths.insert(parent_dir_url_2);

    // Store the path we will first visit
    unvisited_paths.push_front(dl_url.path().to_string());

    // `head` is used to perform HEADER req
    let head = reqwest::blocking::Client::new();

    while !unvisited_paths.is_empty() {
        // Grab the first object stored in queue
        let curr_path = unvisited_paths.pop_back().unwrap();
        // fastdl parent directory link results in no suffix "/" character
        // Check reason on passing in `parent_dir_url_2` into `visited_paths` HashSet
        let curr_path_alt = {
            let mut temp_chars = curr_path.chars();
            temp_chars.next_back();
            temp_chars.as_str().to_string()
        };

        // Add `curr_path` as a visited link
        visited_paths.insert(curr_path.clone());
        visited_paths.insert(curr_path_alt);

        // Create a url out of the `dl_url` &str
        let url = base_url.join(curr_path.as_str())?;

        // GET Request containing all the links to recursively traverse
        let req = reqwest::blocking::get(url.as_str())?.text()?;

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
                if !visited_paths.contains(path)
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
                        unvisited_paths.push_front(path.to_string());
                    } else {
                        // Only add "fastdlv2" in our `download_links` Vec
                        download_links.push(next_site);
                    }
                }
            });

        // DEBUG: Print out the status of every iteration of the loop
        println!("{}\n", "=".repeat(SEP_LEN));
        println!("Status:");
        println!("Visited Paths:\t\t{}", visited_paths.len());
        println!("Unvisited Paths:\t{}", unvisited_paths.len());
        println!("Downloadable Links:\t{}\n", download_links.len());
        println!("{}\n", "=".repeat(SEP_LEN));
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
