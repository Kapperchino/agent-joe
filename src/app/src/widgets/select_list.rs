use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::widgets::{Block, List, ListItem, ListState, StatefulWidget};

pub struct SelectList {
    title: String,
    empty_message: String,
    selected_symbol: String,
    style: Style,
    selected_style: Style,
}

pub struct SelectListState {
    items: Vec<String>,
    selected: Option<usize>,
    list_state: ListState,
}

impl Default for SelectList {
    fn default() -> Self {
        Self {
            title: "Select".to_string(),
            empty_message: "No items".to_string(),
            selected_symbol: "> ".to_string(),
            style: Style::default(),
            selected_style: Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }
}

impl SelectList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
}

impl StatefulWidget for SelectList {
    type State = SelectListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut SelectListState)
    where
        Self: Sized,
    {
        let items = if state.items.is_empty() {
            vec![ListItem::new(self.empty_message).style(Style::default().fg(Color::DarkGray))]
        } else {
            state
                .items
                .iter()
                .map(|item| ListItem::new(item.clone()))
                .collect()
        };

        let list = List::new(items)
            .block(Block::bordered().title(self.title))
            .style(self.style)
            .highlight_style(self.selected_style)
            .highlight_symbol(self.selected_symbol);

        if state.items.is_empty() {
            state.list_state.select(None);
        } else {
            state.sync_list_state();
        }

        StatefulWidget::render(list, area, buf, &mut state.list_state);
    }
}

impl SelectListState {
    pub fn new(items: Vec<String>, selected: &str) -> Self {
        let selected = items.iter().position(|x| x == selected).unwrap_or(0);
        let mut list_state = ListState::default();
        list_state.select(Some(selected));

        Self {
            items,
            selected: Some(selected),
            list_state,
        }
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_item(&self) -> Option<&str> {
        self.selected
            .and_then(|selected| self.items.get(selected))
            .map(String::as_str)
    }

    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        self.selected = self
            .selected
            .filter(|selected| *selected < self.items.len())
            .or_else(|| (!self.items.is_empty()).then_some(0));
        self.sync_list_state();
    }

    pub fn push(&mut self, item: impl Into<String>) {
        self.items.push(item.into());
        if self.selected.is_none() {
            self.selected = Some(0);
        }
        self.sync_list_state();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.selected = None;
        self.sync_list_state();
    }

    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }

        self.selected = Some(index);
        self.sync_list_state();
        true
    }

    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            self.selected = None;
        } else {
            self.selected = Some(
                self.selected
                    .map_or(0, |selected| (selected + 1) % self.items.len()),
            );
        }

        self.sync_list_state();
    }

    pub fn select_previous(&mut self) {
        if self.items.is_empty() {
            self.selected = None;
        } else {
            self.selected = Some(self.selected.map_or(0, |selected| {
                selected.checked_sub(1).unwrap_or(self.items.len() - 1)
            }));
        }

        self.sync_list_state();
    }

    pub fn select_first(&mut self) {
        self.selected = (!self.items.is_empty()).then_some(0);
        self.sync_list_state();
    }

    pub fn select_last(&mut self) {
        self.selected = self.items.len().checked_sub(1);
        self.sync_list_state();
    }

    fn sync_list_state(&mut self) {
        self.list_state.select(self.selected);
    }
}

impl Default for SelectListState {
    fn default() -> Self {
        Self::new(Vec::new(), "")
    }
}
