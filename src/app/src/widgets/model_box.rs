use crate::models::{EffortsSelection, ModelSelections};
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
        self.select_list.render(area, buf, &mut state.list_state);
    }
}

impl ModelBoxState {
    pub fn new(models: ModelSelections, efforts: EffortsSelection) -> ModelBoxState {
        let models = models.get_models();
        let efforts = efforts.get_efforts();
        ModelBoxState {
            models: models.clone(),
            efforts,
            list_state: SelectListState::new(models),
            page_state: PageState::SelectModel,
        }
    }

    pub fn on_arrow_down(&mut self) {
        self.list_state.select_next()
    }

    pub fn on_arrow_up(&mut self) {
        self.list_state.select_previous()
    }

    pub fn on_enter(&mut self) {}
}

impl ModelBox {
    pub fn new() -> ModelBox {
        ModelBox {
            select_list: SelectList::new().title("Model"),
        }
    }
}
