use std::io;
use tui::{
    backend::CrosstermBackend,
    Terminal,
    widgets::{Block, Borders, Paragraph},
    layout::{Layout, Constraint, Direction},
    style::{Style, Color, Modifier},
    text::{Text, Span, Spans},
};
use crossterm::{
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
    event::{read, Event, KeyCode},
};
use scraper::{Html, Selector, ElementRef};
use reqwest::blocking::get;
use url::Url;

struct Link {
    url: String,
    display_text: String,
}

// Recursive parse function using ElementRef
fn parse_element(
    element: &ElementRef,
    links: &mut Vec<Link>,
    spans_vec: &mut Vec<(Spans<'static>, Option<usize>)>,
    base_url: &Url,
) {
    let tag = element.value().name();

    if tag == "a" {
        if let Some(href) = element.value().attr("href") {
            let url = if href.starts_with("http") {
                href.to_string()
            } else {
                base_url.join(href).map(|u| u.to_string()).unwrap_or_else(|_| href.to_string())
            };
            let display_text = element.text().collect::<Vec<_>>().join(" ").trim().to_string();
            if !display_text.is_empty() {
                let link_idx = links.len();
                links.push(Link { url, display_text: display_text.clone() });
                spans_vec.push((Spans::from(Span::raw(display_text)), Some(link_idx)));
            }
        }
    } else {
        // First recurse children nodes (both text and elements)
        for child in element.children() {
            if let Some(child_element) = ElementRef::wrap(child) {
                parse_element(&child_element, links, spans_vec, base_url);
            } else if let Some(text) = child.value().as_text() {
                let text_str = text.text.trim();
                if !text_str.is_empty() {
                    spans_vec.push((Spans::from(Span::raw(text_str.to_string())), None));
                }
            }
        }

        // Now add a line break *only* if the current element is a block element
        if ["p", "div", "br", "li", "ul", "ol", "section", "article"].contains(&tag) {
            spans_vec.push((Spans::from(""), None));
        }
    }
}


fn parse_html(html: &str, base_url: &Url) -> (Vec<Link>, Vec<(Spans<'static>, Option<usize>)>) {
    let document = Html::parse_document(html);
    let body_selector = Selector::parse("body").unwrap();

    let mut links = Vec::new();
    let mut spans_vec = Vec::new();

    if let Some(body) = document.select(&body_selector).next() {
        parse_element(&body, &mut links, &mut spans_vec, base_url);
    }

    (links, spans_vec)
}

fn fetch_url(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let response = get(url)?;
    let body = response.text()?;
    Ok(body)
}

fn display_loop(mut url: String) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let link_style = Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED);
    let selected_style = Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let mut selected_link_idx: Option<usize> = None;
    let mut scroll_offset: u16 = 0;

    // Cache for current page
    let mut current_links: Vec<Link> = Vec::new();
    let mut current_spans: Vec<(Spans<'static>, Option<usize>)> = Vec::new();
    let mut last_url = String::new();

    loop {
        // Fetch and parse only if URL changed
        if url != last_url {
            match fetch_url(&url) {
                Ok(html) => {
                    let base_url = Url::parse(&url)?;
                    let (links, spans_vec) = parse_html(&html, &base_url);
                    current_links = links;
                    current_spans = spans_vec;
                    last_url = url.clone();
                    selected_link_idx = None;
                    scroll_offset = 0;
                }
                Err(e) => {
                    // Show an error message as spans if fetch fails
                    current_links.clear();
                    current_spans = vec![(Spans::from(Span::raw(format!("Error fetching URL: {}", e))), None)];
                    last_url = url.clone();
                    selected_link_idx = None;
                    scroll_offset = 0;
                }
            }
        }

        // Prepare styled lines based on current_spans & selection as before
        let styled_lines: Vec<Spans> = current_spans
            .iter()
            .map(|(spans, link_idx_opt)| {
                if let Some(link_idx) = link_idx_opt {
                    let style = if Some(*link_idx) == selected_link_idx {
                        selected_style
                    } else {
                        link_style
                    };
                    let styled_spans = spans.0.iter()
                        .map(|span| Span::styled(span.content.clone(), style))
                        .collect::<Vec<_>>();
                    Spans::from(styled_spans)
                } else {
                    spans.clone()
                }
            })
            .collect();

        terminal.draw(|f| {
            let size = f.size();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1)].as_ref())
                .split(size);

            let max_scroll = styled_lines.len().saturating_sub(chunks[0].height as usize) as u16;
            if scroll_offset > max_scroll {
                scroll_offset = max_scroll;
            }

            let paragraph = Paragraph::new(Text::from(styled_lines))
                .block(Block::default().title(url.as_str()).borders(Borders::ALL))
                .scroll((scroll_offset, 0));
            f.render_widget(paragraph, chunks[0]);
        })?;

        // Read input event and handle navigation, scrolling, etc.
        if let Event::Key(key) = read()? {
            match key.code {
                KeyCode::Char('q') => break,

                KeyCode::Tab => {
                    if !current_links.is_empty() {
                        selected_link_idx = Some(match selected_link_idx {
                            None => 0,
                            Some(i) => (i + 1) % current_links.len(),
                        });
                    }
                }
                KeyCode::BackTab => {
                    if !current_links.is_empty() {
                        selected_link_idx = Some(match selected_link_idx {
                            None => current_links.len() - 1,
                            Some(i) => if i == 0 { current_links.len() - 1 } else { i - 1 },
                        });
                    }
                }
                KeyCode::Enter => {
                    if let Some(i) = selected_link_idx {
                        if let Some(link) = current_links.get(i) {
                            url = link.url.clone();
                            // Fetch happens next loop iteration because url changed
                        }
                    }
                }
                KeyCode::Down => {
                    scroll_offset = scroll_offset.saturating_add(1);
                }
                KeyCode::Up => {
                    scroll_offset = scroll_offset.saturating_sub(1);
                }
                KeyCode::PageDown => {
                    scroll_offset = scroll_offset.saturating_add(10);
                }
                KeyCode::PageUp => {
                    scroll_offset = scroll_offset.saturating_sub(10);
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;

    Ok(())
}



fn main() {
    let start_url = "https://en.wikipedia.org/wiki/Main_Page".to_string();
    if let Err(e) = display_loop(start_url) {
        eprintln!("Error: {}", e);
    }
}
