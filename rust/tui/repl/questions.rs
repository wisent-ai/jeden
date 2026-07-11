use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::super::view_render::picker_panel;
use super::super::{default_rows, Message, PickerEvent, PickerItem, PickerSpec, PickerState};
use super::{message_block, ReplRenderer};

pub(super) fn prompt_user_question(
    renderer: &mut ReplRenderer,
    streamed: &str,
    question: &str,
    options: &[String],
    columns: usize,
    color: bool,
) -> io::Result<Result<String, String>> {
    if !options.is_empty() {
        let items = options
            .iter()
            .map(|option| PickerItem::action(option.clone(), option.clone()))
            .collect();
        let mut picker = PickerState::new(PickerSpec::new(question, items));
        loop {
            let mut lines = Vec::new();
            if !streamed.trim().is_empty() {
                lines.extend(message_block(
                    &Message::new("assistant", streamed.to_string()),
                    columns,
                    color,
                ));
            }
            lines.extend(picker_panel(&picker, columns, default_rows(), color));
            renderer.flush(&[], &lines)?;
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            match picker.handle_key(key) {
                PickerEvent::Pending => {}
                PickerEvent::Cancelled => return Ok(Err("Question cancelled".into())),
                PickerEvent::Submit(answer) | PickerEvent::Prefill(answer) => {
                    return Ok(Ok(answer))
                }
                PickerEvent::Confirm { .. } => {}
            }
        }
    }

    let mut answer = String::new();
    loop {
        let mut lines = Vec::new();
        if !streamed.trim().is_empty() {
            lines.extend(message_block(
                &Message::new("assistant", streamed.to_string()),
                columns,
                color,
            ));
        }
        lines.extend(message_block(
            &Message::new(
                "system",
                format!("{question}\n\nAnswer: {answer}\n\nEnter submit · Esc cancel"),
            ),
            columns,
            color,
        ));
        renderer.flush(&[], &lines)?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(Err("Question cancelled".into())),
            KeyCode::Backspace => {
                answer.pop();
            }
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n')
                if !answer.trim().is_empty() =>
            {
                return Ok(Ok(answer.trim().to_string()))
            }
            KeyCode::Char(ch)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                answer.push(ch);
            }
            _ => {}
        }
    }
}
