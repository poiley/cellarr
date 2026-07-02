//! Dev helper: read release names on stdin, print `name<TAB>clean_title<TAB>year`
//! using the real parser. Lets a dry-run measure parse quality over a real library
//! without deploying. `cat names.txt | cargo run -p cellarr-parse --example parse_titles`.

use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let p = cellarr_parse::parse_title(&line);
        let title = p.clean_title.unwrap_or_default();
        let year = p.year.map(|y| y.to_string()).unwrap_or_default();
        let _ = writeln!(out, "{line}\t{title}\t{year}");
    }
}
