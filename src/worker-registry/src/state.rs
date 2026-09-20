use super::{Entry, WorkerReport, WorkerStatus, WorkerView};

pub(super) enum ReportCollection {
    Pending,
    Collected,
}

pub(super) enum WorkerState {
    Registered,
    Running,
    Cancelling,
    Finished {
        report: Box<WorkerReport>,
        collection: ReportCollection,
    },
}

pub(super) enum WorkerUpdate {
    Started,
    Cancelled,
    Finished(Box<WorkerReport>),
    Collected,
}

impl WorkerState {
    pub(super) fn restored(view: &WorkerView) -> Self {
        Self::Finished {
            report: Box::new(view.recovered_report()),
            collection: ReportCollection::Collected,
        }
    }

    pub(super) fn status(&self) -> WorkerStatus {
        match self {
            Self::Registered => WorkerStatus::Registered,
            Self::Running => WorkerStatus::Running,
            Self::Cancelling => WorkerStatus::Cancelling,
            Self::Finished { report, .. } => report.status,
        }
    }

    pub(super) fn terminal(&self) -> bool {
        matches!(self, Self::Finished { .. })
    }

    pub(super) fn pending(&self) -> bool {
        !matches!(
            self,
            Self::Finished {
                collection: ReportCollection::Collected,
                ..
            }
        )
    }

    fn apply(&mut self, update: WorkerUpdate) -> bool {
        match (&mut *self, update) {
            (Self::Registered, WorkerUpdate::Started) => {
                *self = Self::Running;
                true
            }
            (Self::Registered | Self::Running, WorkerUpdate::Cancelled) => {
                *self = Self::Cancelling;
                true
            }
            (
                Self::Registered | Self::Running | Self::Cancelling,
                WorkerUpdate::Finished(report),
            ) => {
                *self = Self::Finished {
                    report,
                    collection: ReportCollection::Pending,
                };
                true
            }
            (Self::Finished { collection, .. }, WorkerUpdate::Collected) => {
                *collection = ReportCollection::Collected;
                true
            }
            _ => false,
        }
    }
}

impl Entry {
    pub(super) fn view(&self) -> WorkerView {
        let state = self.updates.borrow();
        WorkerView {
            worker_id: self.id.clone(),
            request: self.request.clone(),
            status: state.status(),
            report: match &*state {
                WorkerState::Finished { report, .. } => Some(report.as_ref().clone()),
                _ => None,
            },
        }
    }

    pub(super) fn update(&self, update: WorkerUpdate) -> bool {
        self.updates.send_if_modified(|state| state.apply(update))
    }

    pub(super) fn collect(&self) -> WorkerView {
        self.update(WorkerUpdate::Collected);
        self.view()
    }
}
