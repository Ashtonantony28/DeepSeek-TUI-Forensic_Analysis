use crate::app::{App, TranscriptEntry};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(f.area());

    draw_status(f, chunks[0], app);
    draw_main(f, chunks[1], app);
    draw_composer(f, chunks[2], app);

    if app.show_palette {
        draw_palette(f, f.area(), app);
    }
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let mode = format!("{:?}", app.mode);
    let line = Line::from(vec![
        Span::styled(
            " agent-tui ".to_string(),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " mode={mode}  provider={}  model={}  ctx={:.0}%  cost=${:.4}  {}",
            app.provider.as_str(),
            app.model,
            app.context_used_ratio * 100.0,
            app.session_cost_usd,
            app.status,
        )),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_main(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(30)])
        .split(area);
    draw_transcript(f, chunks[0], app);
    draw_side_panel(f, chunks[1], app);
}

fn draw_transcript(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();
    for e in &app.transcript {
        match e {
            TranscriptEntry::User(s) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "you  ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(s.clone()),
                ]));
            }
            TranscriptEntry::AssistantText(s) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "ai   ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(s.clone()),
                ]));
            }
            TranscriptEntry::AssistantThinking(s) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "think ",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                    Span::styled(s.clone(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            TranscriptEntry::ToolCall { name, input } => {
                lines.push(Line::from(vec![
                    Span::styled("tool ", Style::default().fg(Color::Yellow)),
                    Span::raw(format!("{name}({input})")),
                ]));
            }
            TranscriptEntry::ToolResult {
                name,
                output,
                is_error,
            } => {
                let style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let preview: String = output.chars().take(160).collect();
                lines.push(Line::from(vec![
                    Span::styled(format!("← {name} "), style),
                    Span::raw(preview),
                ]));
            }
            TranscriptEntry::System(s) => {
                lines.push(Line::from(Span::styled(
                    format!("• {s}"),
                    Style::default().fg(Color::Magenta),
                )));
            }
        }
    }
    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("transcript"))
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_side_panel(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("plan");
    let mut lines: Vec<Line> = Vec::new();
    match &app.plan_goal {
        Some(g) => {
            lines.push(Line::from(vec![
                Span::styled(
                    "goal: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(g.clone()),
            ]));
            lines.push(Line::from(""));
            for (i, item) in app.plan_items.iter().enumerate() {
                let marker = if item.done { "[x] " } else { "[ ] " };
                let style = if item.done {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{:>2}. {marker}", i + 1), style),
                    Span::raw(item.step.clone()),
                ]));
            }
        }
        None => {
            lines.push(Line::from(Span::styled(
                "no plan yet",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "the agent populates this panel via update_plan",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_composer(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("compose ↵ to send, Tab cycles mode, Ctrl+K palette, Ctrl+C cancel, Ctrl+D quit");
    let para = Paragraph::new(app.composer.as_str()).block(block);
    f.render_widget(para, area);
}

fn draw_palette(f: &mut Frame, area: Rect, app: &App) {
    let r = centered_rect(60, 30, area);
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("command palette");
    let para = Paragraph::new(app.palette_input.as_str()).block(block);
    f.render_widget(para, r);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
