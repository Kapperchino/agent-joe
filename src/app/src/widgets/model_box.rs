use crate::widgets::model_box::PageState::SelectEffort;
use crate::widgets::select_list::{SelectList, SelectListState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::StatefulWidget;

pub struct ModelBox {
    select_list: SelectList,
}

enum PageState {
    SelectModel,
    SelectEffort,
}

pub struct ModelBoxState {
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub list_state: SelectListState,
    page_state: PageState,
}

impl StatefulWidget for ModelBox {
    type State = ModelBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        
    }
}

impl ModelBoxState {
    pub fn new(models: Vec<String>, efforts: Vec<String>) -> ModelBoxState {
        ModelBoxState {
            models: models.clone(),
            efforts,
            list_state: SelectListState::new(models),
            page_state: SelectEffort,
        }
    }
}

impl ModelBox {
    pub fn new() -> ModelBox {
        ModelBox {
            select_list: Default::default(),
        }
    }
}
