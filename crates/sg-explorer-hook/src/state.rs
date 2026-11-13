//! The DLL's state machine (pure logic, testable anywhere).
//!
//! - `Idle`: no app yet (or it has not said hello). Paints nothing.
//! - `Active`: the app said hello with the right version; `labels` = the label paint table currently applied (None = Windows default).
//! - `Resting`: rests until Explorer restarts (version mismatch, panic). Never paints again,
//!   but still answers the app so the app can tell the user why.

use sg_explorer_proto::{FromDll, LabelsPaint, PROTO_VERSION};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum DllState {
    Idle,
    Active { labels: Option<Arc<LabelsPaint>> },
    Resting(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    AppHello { version: u32 },
    Labels(LabelsPaint),
    LabelsOff,
    Disconnected,
    /// Severe error (panic) on the desktop thread or the pipe thread.
    Fault(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Paint table the desktop thread must apply (None = restore the default).
    Paint(Option<Arc<LabelsPaint>>),
    Reply(FromDll),
}

/// Resting reason for a protocol version mismatch (the app reads this string to report "Explorer needs to be restarted").
pub const VERSION_MISMATCH: &str = "version-mismatch";

fn status(labels_active: bool, error: Option<String>) -> Effect {
    Effect::Reply(FromDll::Status { labels_active, error })
}

impl DllState {
    pub fn is_resting(&self) -> bool {
        matches!(self, DllState::Resting(_))
    }

    pub fn labels(&self) -> Option<&Arc<LabelsPaint>> {
        match self {
            DllState::Active { labels } => labels.as_ref(),
            _ => None,
        }
    }

    pub fn step(self, input: Input) -> (DllState, Vec<Effect>) {
        use DllState::*;
        match (self, input) {
            // Resting: no more painting; losing the connection still reliably restores the default.
            (Resting(r), Input::Disconnected) => (Resting(r), vec![Effect::Paint(None)]),
            (Resting(r), Input::Fault(_)) => (Resting(r), vec![]),
            (Resting(r), _) => {
                let e = status(false, Some(r.clone()));
                (Resting(r), vec![e])
            }
            (_, Input::Fault(msg)) => {
                let r = format!("fault: {msg}");
                (Resting(r.clone()), vec![Effect::Paint(None), status(false, Some(r))])
            }
            (_, Input::Disconnected) => (Idle, vec![Effect::Paint(None)]),
            (_, Input::AppHello { version }) if version != PROTO_VERSION => {
                let r = VERSION_MISMATCH.to_string();
                (Resting(r.clone()), vec![Effect::Paint(None), status(false, Some(r))])
            }
            // Hello again mid-session: keep the table currently applied (the app resends the state right after).
            (Active { labels }, Input::AppHello { .. }) => (Active { labels }, vec![]),
            (Idle, Input::AppHello { .. }) => (Active { labels: None }, vec![]),
            (Active { .. }, Input::Labels(p)) => {
                let p = Arc::new(p.sanitized());
                (Active { labels: Some(p.clone()) }, vec![Effect::Paint(Some(p)), status(true, None)])
            }
            (Active { .. }, Input::LabelsOff) => (Active { labels: None }, vec![Effect::Paint(None), status(false, None)]),
            (Idle, Input::Labels(_) | Input::LabelsOff) => (Idle, vec![status(false, Some("hello-required".into()))]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sg_explorer_proto::{PaintMode, TextStyle};

    fn paint() -> LabelsPaint {
        LabelsPaint { mode: PaintMode::Uniform, uniform: Some(TextStyle::plain(0xFF00FF)), items: vec![] }
    }

    fn paints(eff: &[Effect]) -> Vec<Option<u32>> {
        eff.iter()
            .filter_map(|e| match e {
                Effect::Paint(p) => Some(p.as_ref().and_then(|p| p.uniform.map(|u| u.color))),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn state_transitions() {
        let (s, e) = DllState::Idle.step(Input::Labels(paint()));
        assert_eq!(s, DllState::Idle, "chưa chào thì không nhận lệnh vẽ");
        assert!(paints(&e).is_empty());

        let (s, e) = DllState::Idle.step(Input::AppHello { version: PROTO_VERSION });
        assert!(matches!(s, DllState::Active { labels: None }));
        assert!(e.is_empty());

        let (s, e) = s.step(Input::Labels(paint()));
        assert_eq!(paints(&e), vec![Some(0xFF00FF)]);
        assert_eq!(s.labels().unwrap().uniform.unwrap().color, 0xFF00FF);

        let (s2, e) = s.clone().step(Input::LabelsOff);
        assert_eq!(paints(&e), vec![None]);
        assert!(s2.labels().is_none());

        // Pipe lost → Idle, restore the default.
        let (s, e) = s.step(Input::Disconnected);
        assert_eq!(s, DllState::Idle);
        assert_eq!(paints(&e), vec![None]);

        // Panic → rest for good; afterwards paint commands are refused and only Status is answered.
        let (s, _) = DllState::Idle.step(Input::AppHello { version: PROTO_VERSION });
        let (s, e) = s.step(Input::Fault("boom".into()));
        assert!(s.is_resting());
        assert_eq!(paints(&e), vec![None]);
        let (s, e) = s.step(Input::Labels(paint()));
        assert!(s.is_resting());
        assert!(paints(&e).is_empty());
        assert!(matches!(&e[..], [Effect::Reply(FromDll::Status { error: Some(_), .. })]));
        let (s, e) = s.step(Input::Disconnected);
        assert!(s.is_resting(), "nghỉ là nghỉ tới khi Explorer khởi động lại");
        assert_eq!(paints(&e), vec![None]);
    }

    #[test]
    fn hello_version_mismatch_rests() {
        let (s, e) = DllState::Idle.step(Input::AppHello { version: PROTO_VERSION + 1 });
        assert_eq!(s, DllState::Resting(VERSION_MISMATCH.into()));
        assert_eq!(paints(&e), vec![None]);
        assert!(e.iter().any(|x| matches!(x, Effect::Reply(FromDll::Status { error: Some(r), .. }) if r == VERSION_MISMATCH)));
        // Reconnecting with an app of the right version still does not paint (the old DLL must yield to the new one after Explorer restarts).
        let (s, _) = s.step(Input::Disconnected);
        let (s, e) = s.step(Input::AppHello { version: PROTO_VERSION });
        assert!(s.is_resting());
        let (_, e2) = s.step(Input::Labels(paint()));
        assert!(paints(&e).is_empty() && paints(&e2).is_empty());
    }

    #[test]
    fn rehello_keeps_current_paint() {
        let (s, _) = DllState::Idle.step(Input::AppHello { version: PROTO_VERSION });
        let (s, _) = s.step(Input::Labels(paint()));
        let (s, e) = s.step(Input::AppHello { version: PROTO_VERSION });
        assert!(e.is_empty(), "không trả mặc định rồi vẽ lại (tránh nháy)");
        assert!(s.labels().is_some());
    }
}
