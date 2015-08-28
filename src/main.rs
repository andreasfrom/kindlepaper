extern crate sqlite3;
extern crate time;

use std::io::prelude::*;
use std::io::Error;
use std::fs::{self, File, metadata, create_dir};
use std::path::Path;
use std::process::Command;
use std::env::temp_dir;

use sqlite3::{DatabaseConnection, Query, ResultRowAccess, SqliteResult};
use sqlite3::access::open;
use std::iter::FromIterator;

use time::now;

const DATE_FMT: &'static str = "%F"; // ISO 8601

const OUT: &'static str = "out";
const TOC_FILE: &'static str = "toc.html";
const CONTENT_FILE: &'static str = "content.html";

const EXTRACT_SCRIPT: &'static [u8; 122] =
    b"dd if=$1 bs=1 skip=24 | python -c \"import zlib,sys;sys.stdout.write(zlib.decompress(sys.stdin.read()))\" | tar -xvf - -C $2";

#[derive(Debug)]
struct Article {
    title: String,
    byline: String,
    blurb: String,
    content: String,
}

#[derive(Debug)]
struct Config {
    name: String,
    app_id: String,
    select_stmt: String,
}

impl Config {
    fn new(name: &str, app_id: &str, select_stmt: &str) -> Config {
        Config {
            name: name.to_string(),
            app_id: app_id.to_string(),
            select_stmt: select_stmt.to_string(),
        }
    }
}

fn main() {
    let papers = vec![
        Config::new(
            "Politiken",
            "dk.politiken.reader",
            "SELECT title, byline, blurb, content FROM articles WHERE refid LIKE ?"),

        Config::new(
            "Information",
            "dk.information.areader",
            "SELECT title, author AS byline, blurb, content FROM articles LEFT JOIN byline ON articles.article_id == byline.article_id WHERE refid LIKE ?")
        ];

    let ids: Vec<&str> = papers.iter().map(|p| &*p.app_id).collect();

    create_dir(OUT).ok();

    fetch_data_from_android(&ids).unwrap();
    convert_papers(&papers).unwrap();
}

fn convert_papers(papers: &[Config]) -> Result<(), Error> {
    let dir = temp_dir().join("apps");

    for paper in papers {
        let path = dir.join(Path::new(&paper.app_id));
        if is_dir(&path) {
            let db = dir
                .join(Path::new(&paper.app_id))
                .join(Path::new("db"))
                .join(Path::new("data").with_extension("db"));

            let articles = fetch_articles(&db.to_string_lossy(), &paper.select_stmt).unwrap();

            try!(write_toc(&articles));
            try!(write_articles(&articles, &paper.name));
            try!(write_opf(&paper.name));

            kindlegen(&paper.name);
        }
    }

    Ok(())
}

fn fetch_data_from_android(app_ids: &[&str]) -> Result<(), Error> {
    let path = temp_dir().join(Path::new("papers").with_extension("ab"));

    Command::new("adb")
        .arg("backup")
        .arg("-f")
        .arg(&path)
        .arg("-noapk")
        .args(app_ids)
        .status()
        .unwrap_or_else(|e| { panic!("failed to execute process: {}", e) });

    let extract = temp_dir().join(Path::new("extract").with_extension("sh"));

    let mut f = try!(File::create(&extract));
    try!(f.write_all(EXTRACT_SCRIPT));

    Command::new("sh")
        .arg(extract)
        .arg(&path)
        .arg(&temp_dir())
        .status().unwrap();

    Ok(())
}

fn is_dir(path: &Path) -> bool {
    metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn kindlegen(name: &str) {
    let file = temp_dir().join(Path::new(name).with_extension("opf"));
    let out = Path::new(name).with_extension("mobi");

    Command::new("kindlegen")
        .arg(file)
        .output()
        .unwrap_or_else(|e| { panic!("failed to execute process: {}", e) });

    fs::rename(temp_dir().join(&out), Path::new(OUT).join(&out)).unwrap();
}

fn make_name(s: &str) -> String {
    s.replace(" ", "_")
}

fn write_toc(articles: &[Article]) -> Result<(), Error> {
    let path = temp_dir().join(TOC_FILE);
    let mut f = try!(File::create(path));

    try!(f.write_all(b"<!DOCTYPE html>"));
    try!(f.write_all(b"<html>"));
    try!(f.write_all(b"<head>"));
    try!(f.write_all(b"<meta http-equiv=\"content-type\" content=\"text/html; charset=UTF-8\">"));
    try!(f.write_all(b"</head>"));
    try!(f.write_all(b"<body>"));

    try!(f.write_all(b"<nav epub:type=\"toc\">"));
    try!(f.write_all(b"<ol>"));

    for a in articles.iter().filter(|a| a.title != "") {
        try!(write!(f, "<li><a href=\"{}#{}\">{}</a></li>", CONTENT_FILE, make_name(&a.title), a.title));
    }

    try!(f.write_all(b"</ol>"));
    try!(f.write_all(b"</nav>"));
    try!(f.write_all(b"</body>"));
    try!(f.write_all(b"</html>"));

    Ok(())
}

fn write_articles(articles: &[Article], title: &str) -> Result<(), Error> {
    let path = temp_dir().join(CONTENT_FILE);
    let mut f = try!(File::create(path));

    try!(f.write_all(b"<!DOCTYPE html>"));
    try!(f.write_all(b"<html>"));
    try!(f.write_all(b"<head>"));
    try!(write!(f, "<title>{}</title>", title));
    try!(f.write_all(b"<meta http-equiv=\"content-type\" content=\"text/html; charset=UTF-8\">"));
    try!(f.write_all(b"</head>"));
    try!(f.write_all(b"<body>"));

    for a in articles {
        try!(f.write_all(b"<article>"));

        try!(f.write_all(b"<header><div>"));
        try!(write!(f, "<a name=\"{}\"><h1>{}</h1></a>", make_name(&a.title), a.title));
        try!(write!(f, "<h2>{}</h2>", a.blurb));
        try!(f.write_all(b"</div></header>"));

        try!(write!(f, "<address>{}</address>", a.byline));

        try!(f.write_all(b"<section>"));

        for line in a.content.lines() {
            let start: String = FromIterator::from_iter(line.chars().take(16));
            if start == "<div class='h3'>" {
                try!(f.write_all(b"</section>"));
                try!(f.write_all(b"<section>"));

                let len = line.len();
                let end = line.find("</div>").unwrap();

                try!(write!(f, "<h3>{}</h3>", &line[16..end]));

                try!(write!(f, "{}", &line[end+6..len]));
            } else {
                try!(write!(f, "{}", line));
            }

        }

        try!(f.write_all(b"</section>"));
        try!(f.write_all(b"</article>"));
    }

    try!(f.write_all(b"</body>"));
    try!(f.write_all(b"</html>"));

    Ok(())
}

fn fetch_refid_pattern(conn: &DatabaseConnection) -> SqliteResult<String> {
    let mut last_refid_stmt = try!(conn.prepare(
        "SELECT refid FROM articles ORDER BY article_id DESC LIMIT 1"));

    let refid: String;

    let mut res_set = last_refid_stmt.execute();
    match try!(res_set.step()) {
        Some(mut row) => {
            refid = row.get("refid");
        },
        None => panic!("no articles"),
    }

    let sub_refid: String = FromIterator::from_iter(refid.chars().take(6));
    Ok(sub_refid + "%")
}

fn fetch_articles(db_file: &str, select_stmt: &str) -> SqliteResult<Vec<Article>> {
    let conn = try!(open(db_file, None));

    let pattern = try!(fetch_refid_pattern(&conn));

    let mut stmt = try!(conn.prepare(select_stmt));

    let mut articles = vec!();
    try!(stmt.query(
        &[&pattern], &mut |row| {
            articles.push(Article {
                title: row.get("title"),
                byline: row.get("byline"),
                blurb: row.get("blurb"),
                content: row.get("content"),
            });
            Ok(())
        }));

    Ok(articles)
}

fn write_opf(title: &str) -> Result<(), Error> {
    let path = temp_dir().join(Path::new(title).with_extension("opf"));
    let mut f = try!(File::create(path));

    try!(write!(f, "
<?xml version=\"1.0\" encoding=\"utf-8\"?>
<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\" unique-identifier=\"{}\">
  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">
    <dc:title>{}</dc:title>
    <dc:language>da-dk</dc:language>
    <dc:creator>Andreas H. From</dc:creator>
    <dc:publisher>{}</dc:publisher>
    <dc:subject>Newspaper</dc:subject>
    <dc:date>{}</dc:date>
  </metadata>
  <manifest>
    <item id=\"toc\" properties=\"nav\" href=\"{}\" media-type=\"text/html\"/>
    <item id=\"content\" media-type=\"text/html\" href=\"{}\"></item>
  </manifest>
  <spine toc=\"toc\">
    <itemref idref=\"content\"/>
  </spine>
  <guide>
    <reference type=\"toc\" title=\"Table of Contents\" href=\"toc.html\"></reference>
  </guide>
</package>",
                title, title, title,
                now().strftime(DATE_FMT).unwrap(),
                TOC_FILE, CONTENT_FILE));

    Ok(())
}
