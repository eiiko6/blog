use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use clap::{Parser, Subcommand};
use lazy_static::lazy_static;
use pulldown_cmark::{Options, Parser as MarkdownParser, html};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{io::Cursor, path::PathBuf};
use syntect::{highlighting::ThemeSet, parsing::SyntaxSet};
use tera::{Context, Tera};
use tokio::io::{AsyncBufReadExt, BufReader};

mod codeblocks;
use codeblocks::*;

lazy_static! {
    pub static ref TEMPLATES: Tera = {
        let mut tera = Tera::default();
        tera.add_raw_templates(vec![
            ("_base.html", include_str!("../templates/_base.html")),
            ("home.html", include_str!("../templates/home.html")),
            ("page.html", include_str!("../templates/page.html")),
            ("style.css", include_str!("../templates/style.css")),
        ])
        .unwrap();
        tera
    };
    pub static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    pub static ref THEME_SET: ThemeSet = {
        let mut set = ThemeSet::load_defaults();

        let theme_bytes = include_bytes!("../themes/Catppuccin-Macchiato.tmTheme");

        let mut cursor = Cursor::new(theme_bytes);
        match syntect::highlighting::ThemeSet::load_from_reader(&mut cursor) {
            Ok(theme) => {
                set.themes.insert("Catppuccin Macchiato".to_string(), theme);
            }
            Err(e) => {
                tracing::error!("Failed to load embedded theme: {}", e);
            }
        }
        set
    };
}

#[derive(Parser)]
#[command(author, version, about = "A simple markdown book server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Serve the markdown files in a directory
    Serve {
        /// Path to the directory containing SUMMARY.md
        path: PathBuf,
        /// Port to listen on
        #[arg(short, long, default_value = "3456")]
        port: u16,
    },
}

struct AppState {
    docs_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lazy_static::initialize(&TEMPLATES);

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { path, port } => {
            let abs_path = std::fs::canonicalize(&path)?;

            let shared_state = Arc::new(AppState { docs_dir: abs_path });

            let app = Router::new()
                .route("/", get(render_summary))
                .route("/{page}", get(render_page))
                .route("/style.css", get(serve_css))
                .with_state(shared_state);

            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
            tracing::info!("Listening on http://localhost:{}", port);
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}

async fn render_summary(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut context = Context::new();
    context.insert("title", "Pages");

    #[derive(Deserialize, Serialize)]
    struct Page {
        filename: String,
        title: String,
        datetime: String,
    }

    let mut pages: Vec<Page> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&state.docs_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let filename = entry.file_name();
            let filename = filename.to_str().unwrap_or("<filename>");

            let title = if let Ok(file) = tokio::fs::File::open(&path).await {
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(_) => line.trim_start_matches('#').trim().to_string(),
                    Err(_) => filename.to_string(),
                }
            } else {
                filename.to_string()
            };

            let datetime = filename
                .split_once('@')
                .and_then(|(_, ts_with_ext)| ts_with_ext.split('.').next())
                .map(|dt| dt.to_string())
                .unwrap_or_else(|| "Invalid Date".to_string());

            pages.push(Page {
                filename: filename.to_string(),
                title,
                datetime,
            });
        }
    }

    context.insert("files", &pages);

    match TEMPLATES.render("home.html", &context) {
        Ok(rendered) => Html(rendered),
        Err(e) => Html(format!("<h1>Template Error</h1><pre>{}</pre>", e)),
    }
}

async fn render_page(
    State(state): State<Arc<AppState>>,
    Path(page): Path<String>,
) -> impl IntoResponse {
    let filename = if page.ends_with(".md") {
        page
    } else {
        format!("{}.md", page)
    };

    let file_path = state.docs_dir.join(&filename);

    let content = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(_) => return Html("<h1>404</h1><p>Page not found</p>".to_string()),
    };
    render_md_file(&content, &filename, state).await
}

async fn serve_css() -> impl IntoResponse {
    match TEMPLATES.render("style.css", &tera::Context::new()) {
        Ok(css) => Response::builder()
            .header("content-type", "text/css")
            .body(css.into())
            .unwrap(),
        Err(_) => (StatusCode::NOT_FOUND, "CSS not found").into_response(),
    }
}

async fn render_md_file(content: &String, filename: &str, state: Arc<AppState>) -> Html<String> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = MarkdownParser::new_ext(&content, options);

    let renderer = CodeblockRenderer::new(parser);

    let mut html_output = String::new();
    html::push_html(&mut html_output, renderer);

    let (prev_page, next_page) = get_nav_links(&state.docs_dir, filename);

    let mut context = Context::new();
    context.insert("title", filename);
    context.insert("content", &html_output);
    context.insert("prev_page", &prev_page);
    context.insert("next_page", &next_page);

    match TEMPLATES.render("page.html", &context) {
        Ok(rendered) => Html(rendered),
        Err(e) => Html(format!("<h1>Template Error</h1><pre>{}</pre>", e)),
    }
}

fn get_nav_links(dir: &PathBuf, current_file: &str) -> (Option<String>, Option<String>) {
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? == "md" && path.file_name()? != "SUMMARY.md" {
                Some(path.file_name()?.to_str()?.to_string())
            } else {
                None
            }
        })
        .collect();

    files.sort();

    if current_file == "SUMMARY.md" {
        return (None, files.first().cloned());
    }

    let pos = files.iter().position(|f| f == current_file);

    match pos {
        Some(i) => {
            let prev = if i == 0 {
                // If first page, point back to summary page
                Some(".".to_string())
            } else {
                // Otherwise point to the previous file in the list
                files.get(i - 1).cloned()
            };

            let next = files.get(i + 1).cloned();
            (prev, next)
        }

        None => (None, None),
    }
}
