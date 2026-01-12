use ax_models::Page;
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
        let theme_bytes = include_bytes!(env!("THEME_FILE_PATH"));
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
#[command(author, version, about = "A simple markdown book server/builder")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Serve the markdown files dynamically
    Serve {
        /// Path to the directory containing markdown files
        path: PathBuf,

        /// Whether the home page and navbar should be removed
        #[arg(short, long)]
        no_navigation: bool,

        /// Port to listen on
        #[arg(short, long, default_value = "3456")]
        port: u16,

        /// Whether to serve on 0.0.0.0 (local network)
        #[arg(short = 'H', long)]
        host: bool,
    },
    /// Build static HTML files from the markdown directory
    Build {
        /// Path to the directory containing markdown files
        path: PathBuf,

        /// Whether the home page and navbar should be removed
        #[arg(short, long)]
        no_navigation: bool,

        /// Output directory (defaults to the input directory)
        #[arg(short, long)]
        out_dir: Option<PathBuf>,
    },
}

struct AppState {
    docs_dir: PathBuf,
    no_navigation: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lazy_static::initialize(&TEMPLATES);
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            path,
            port,
            host,
            no_navigation,
        } => {
            let abs_path = std::fs::canonicalize(&path)?;
            let shared_state = Arc::new(AppState {
                docs_dir: abs_path,
                no_navigation,
            });
            let app = Router::new()
                .route("/", get(render_summary_handler))
                .route("/{page}", get(render_page_handler))
                .route("/style.css", get(serve_css))
                .with_state(shared_state);

            let addr = if host {
                format!("0.0.0.0:{}", port)
            } else {
                format!("127.0.0.1:{}", port)
            };
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tracing::info!("Listening on http://{}", addr);
            axum::serve(listener, app).await?;
        }
        Commands::Build {
            path,
            no_navigation,
            out_dir,
        } => {
            let abs_path = std::fs::canonicalize(&path)?;
            let output_path = out_dir.unwrap_or_else(|| abs_path.clone());
            tokio::fs::create_dir_all(&output_path).await?;

            run_build(abs_path, output_path, no_navigation).await?;
        }
    }
    Ok(())
}

async fn get_summary_data(docs_dir: &PathBuf) -> Vec<Page> {
    let mut pages = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(docs_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }

            let filename = entry.file_name();
            let filename_str = filename.to_str().unwrap_or("");

            let title = if let Ok(file) = tokio::fs::File::open(&path).await {
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(_) => line.trim_start_matches('#').trim().to_string(),
                    Err(_) => filename_str.to_string(),
                }
            } else {
                filename_str.to_string()
            };

            let datetime = filename_str
                .split_once('@')
                .and_then(|(_, ts_with_ext)| ts_with_ext.split('.').next())
                .map(|dt| dt.to_string())
                .unwrap_or_else(|| "Invalid Date".to_string());

            pages.push(Page {
                filename: filename_str.to_string(),
                title,
                datetime,
            });
        }
    }
    pages.sort_by(|a, b| b.datetime.cmp(&a.datetime));
    pages
}

async fn render_markdown_to_html(
    content: &str,
    filename: &str,
    docs_dir: &PathBuf,
    no_navigation: bool,
    is_static: bool,
) -> String {
    let mut options = Options::empty();
    options.insert(
        Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS,
    );

    let parser = MarkdownParser::new_ext(content, options);
    let renderer = CodeblockRenderer::new(parser);
    let mut html_output = String::new();
    html::push_html(&mut html_output, renderer);

    let (mut prev, mut next) = if no_navigation {
        (None, None)
    } else {
        get_nav_links(docs_dir, filename)
    };

    // If building statically, rewrite .md links to .html
    if is_static {
        prev = prev.map(|s| {
            if s == "." {
                "index.html".to_string()
            } else {
                s.replace(".md", ".html")
            }
        });
        next = next.map(|s| s.replace(".md", ".html"));
    }

    let mut context = Context::new();
    context.insert("title", filename);
    context.insert("content", &html_output);
    context.insert("prev_page", &prev);
    context.insert("next_page", &next);
    context.insert("no_navigation", &no_navigation);
    context.insert("is_static", &is_static);

    TEMPLATES
        .render("page.html", &context)
        .unwrap_or_else(|e| format!("Error: {}", e))
}

async fn run_build(docs_dir: PathBuf, out_dir: PathBuf, no_navigation: bool) -> anyhow::Result<()> {
    tracing::info!("Building static site to: {:?}", out_dir);

    // Build summary
    if !no_navigation {
        let pages = get_summary_data(&docs_dir).await;
        // Rewrite filenames for static links in home page
        let static_pages: Vec<Page> = pages
            .into_iter()
            .map(|mut p| {
                p.filename = p.filename.replace(".md", ".html");
                p
            })
            .collect();

        let mut context = Context::new();
        context.insert("title", "Pages");
        context.insert("files", &static_pages);
        context.insert("is_static", &true);

        let rendered = TEMPLATES.render("home.html", &context)?;
        tokio::fs::write(out_dir.join("index.html"), rendered).await?;
    }

    // Build css
    let css = TEMPLATES.render("style.css", &Context::new())?;
    tokio::fs::write(out_dir.join("style.css"), css).await?;

    // Build pages
    let mut entries = tokio::fs::read_dir(&docs_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("md") {
            let filename = entry.file_name().to_str().unwrap().to_string();
            let content = tokio::fs::read_to_string(&path).await?;
            let rendered =
                render_markdown_to_html(&content, &filename, &docs_dir, no_navigation, true).await;

            let out_file = out_dir.join(filename.replace(".md", ".html"));
            tokio::fs::write(out_file, rendered).await?;
            tracing::info!("Generated {}", filename);
        }
    }

    tracing::info!("Build complete!");
    Ok(())
}

async fn render_summary_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.no_navigation {
        return (StatusCode::NOT_FOUND, "Disabled").into_response();
    }
    let pages = get_summary_data(&state.docs_dir).await;
    let mut context = Context::new();
    context.insert("title", "Pages");
    context.insert("files", &pages);
    context.insert("is_static", &false);

    match TEMPLATES.render("home.html", &context) {
        Ok(rendered) => Html(rendered).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn render_page_handler(
    State(state): State<Arc<AppState>>,
    Path(page): Path<String>,
) -> impl IntoResponse {
    let filename = if page.ends_with(".md") {
        page
    } else {
        format!("{}.md", page)
    };
    let file_path = state.docs_dir.join(&filename);

    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => Html(
            render_markdown_to_html(
                &content,
                &filename,
                &state.docs_dir,
                state.no_navigation,
                false,
            )
            .await,
        ),
        Err(_) => Html("<h1>404</h1><p>Page not found</p>".to_string()),
    }
}

async fn serve_css() -> impl IntoResponse {
    match TEMPLATES.render("style.css", &Context::new()) {
        Ok(css) => Response::builder()
            .header("content-type", "text/css")
            .body(css.into())
            .unwrap(),
        Err(_) => (StatusCode::NOT_FOUND, "CSS not found").into_response(),
    }
}

// Helper model for Tera
mod ax_models {
    use serde::{Deserialize, Serialize};
    #[derive(Deserialize, Serialize, Clone)]
    pub struct Page {
        pub filename: String,
        pub title: String,
        pub datetime: String,
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
    let pos = files.iter().position(|f| f == current_file);
    match pos {
        Some(i) => {
            let prev = if i == 0 {
                Some(".".to_string())
            } else {
                files.get(i - 1).cloned()
            };
            let next = files.get(i + 1).cloned();
            (prev, next)
        }
        None => (None, None),
    }
}
