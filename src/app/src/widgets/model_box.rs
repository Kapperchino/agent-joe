use crate::models::{EffortsSelection, ModelSelections};
use crate::widgets::select_list::{SelectList, SelectListState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::StatefulWidget;

pub struct ModelBox {
    select_list: SelectList,
}

#[derive(Debug, Clone)]
pub enum ModelBoxPageState {
    SelectModel,
    SelectEffort,
}

#[derive(Debug, Clone)]
pub enum ModelBoxResult {
    SelectModel,
    SelectEffort(String, String),
}

pub struct ModelBoxState {
    pub models: Vec<String>,
    pub model: String,
    pub effort: String,
    pub efforts: Vec<String>,
    pub list_state: SelectListState,
    page_state: ModelBoxPageState,
}

impl StatefulWidget for ModelBox {
    type State = ModelBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        self.select_list.render(area, buf, &mut state.list_state);
    }
}

impl ModelBoxState {
    pub fn new(
        models: ModelSelections,
        efforts: EffortsSelection,
        model_name: String,
        effort: String,
    ) -> ModelBoxState {
        let models = models.get_models();
        let efforts = efforts.get_efforts();
        let mut list_state = SelectListState::new(models.clone(), &model_name);
        list_state.select_first();
        ModelBoxState {
            model: model_name,
            models,
            effort,
            efforts,
            list_state,
            page_state: ModelBoxPageState::SelectModel,
        }
    }

    pub fn height(&self) -> u16 {
        let content_height = self.list_state.len().max(1).saturating_add(2);
        u16::try_from(content_height).unwrap_or(u16::MAX)
    }

    pub fn on_arrow_down(&mut self) {
        self.list_state.select_next()
    }

    pub fn on_arrow_up(&mut self) {
        self.list_state.select_previous()
    }

    pub fn on_enter(&mut self) -> ModelBoxResult {
        match &self.page_state {
            ModelBoxPageState::SelectModel => {
                self.model = self.list_state.selected_item().unwrap_or("").to_string();
                self.list_state.set_items(self.efforts.clone());
                self.update_state(ModelBoxPageState::SelectEffort);
                ModelBoxResult::SelectModel
            }
            ModelBoxPageState::SelectEffort => {
                self.effort = self.list_state.selected_item().unwrap_or("").to_string();
                self.list_state.set_items(self.models.clone());
                self.update_state(ModelBoxPageState::SelectModel);
                ModelBoxResult::SelectEffort(self.model.clone(), self.effort.clone())
            }
        }
    }

    fn update_state(&mut self, new_state: ModelBoxPageState) {
        self.page_state = new_state
    }
}

impl ModelBox {
    pub fn new() -> ModelBox {
        ModelBox {
            select_list: SelectList::new().title("Model"),
        }
    }
}
