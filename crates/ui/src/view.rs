use retrovert_host::visualization::VizSnapshot;

use crate::fasttracker_view::FastTrackerView;
use crate::general_view::GeneralView;
use crate::pt_view::PtView;
use crate::sid_view::SidView;
use crate::tfmx_view::TfmxView;
use crate::v2m_view::V2mView;
use crate::vgm_view::VgmView;

pub const VIEW_COUNT: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum ViewKind {
    ProTracker,
    General,
    FastTracker,
    Sid,
    Tfmx,
    Vgm,
    V2m,
}

#[cfg(test)]
const ALL_VIEW_KINDS: [ViewKind; VIEW_COUNT] = [
    ViewKind::ProTracker,
    ViewKind::General,
    ViewKind::FastTracker,
    ViewKind::Sid,
    ViewKind::Tfmx,
    ViewKind::Vgm,
    ViewKind::V2m,
];

pub trait View: Send {
    fn render(&mut self, snapshot: &VizSnapshot);
}

pub struct Views {
    slots: [Option<Box<dyn View>>; VIEW_COUNT],
}

impl Views {
    pub fn new(
        protracker: PtView,
        general: GeneralView,
        fasttracker: FastTrackerView,
        sid: SidView,
        tfmx: TfmxView,
        vgm: VgmView,
        v2m: V2mView,
    ) -> Self {
        Self::from_views([
            Box::new(protracker),
            Box::new(general),
            Box::new(fasttracker),
            Box::new(sid),
            Box::new(tfmx),
            Box::new(vgm),
            Box::new(v2m),
        ])
    }

    fn from_views(views: [Box<dyn View>; VIEW_COUNT]) -> Self {
        Self {
            slots: views.map(Some),
        }
    }

    #[cfg(test)]
    fn with_view(kind: ViewKind, view: Box<dyn View>) -> Self {
        let mut slots: [Option<Box<dyn View>>; VIEW_COUNT] = ALL_VIEW_KINDS.map(|_| None);
        slots[kind as usize] = Some(view);
        Self { slots }
    }

    pub fn get_mut(&mut self, kind: ViewKind) -> Option<&mut (dyn View + '_)> {
        let slot = if self.slots[kind as usize].is_some() {
            kind as usize
        } else {
            ViewKind::General as usize
        };
        match self.slots[slot].as_mut() {
            Some(view) => Some(view.as_mut()),
            None => None,
        }
    }

    pub fn select(plugin_name: &str, extension: &str, channels: usize) -> ViewKind {
        if plugin_name.eq_ignore_ascii_case("sidplayfp") {
            return ViewKind::Sid;
        }
        if plugin_name.eq_ignore_ascii_case("tfmx") {
            return ViewKind::Tfmx;
        }
        if plugin_name.eq_ignore_ascii_case("libvgm") {
            return ViewKind::Vgm;
        }
        if plugin_name.eq_ignore_ascii_case("v2m") {
            return ViewKind::V2m;
        }
        if plugin_name.eq_ignore_ascii_case("libopenmpt") {
            if extension.eq_ignore_ascii_case("mod") && channels > 0 && channels <= 4 {
                return ViewKind::ProTracker;
            }
            if ["xm", "s3m", "it", "mptm"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            {
                return ViewKind::FastTracker;
            }
        }
        ViewKind::General
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyView;

    impl View for DummyView {
        fn render(&mut self, _snapshot: &VizSnapshot) {}
    }

    #[test]
    fn dispatch_is_a_flat_seven_slot_table() {
        assert_eq!(VIEW_COUNT, 7);
        assert_eq!(ViewKind::ProTracker as usize, 0);
        assert_eq!(ViewKind::V2m as usize, VIEW_COUNT - 1);
        assert_eq!(
            std::mem::size_of::<ViewKind>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn protracker_slot_and_selection_are_explicit() {
        let mut views = Views::with_view(ViewKind::ProTracker, Box::new(DummyView));
        assert!(views.get_mut(ViewKind::ProTracker).is_some());
        assert!(views.get_mut(ViewKind::General).is_none());
        assert_eq!(Views::select("libopenmpt", "mod", 1), ViewKind::ProTracker);
        assert_eq!(Views::select("libopenmpt", "MOD", 4), ViewKind::ProTracker);
        assert_eq!(Views::select("libopenmpt", "mod", 0), ViewKind::General);
        assert_eq!(Views::select("libopenmpt", "mod", 5), ViewKind::General);
        assert_eq!(Views::select("libopenmpt", "xm", 4), ViewKind::FastTracker);
        assert_eq!(
            Views::select("libopenmpt", "S3M", 16),
            ViewKind::FastTracker
        );
        assert_eq!(Views::select("libopenmpt", "it", 64), ViewKind::FastTracker);
        assert_eq!(
            Views::select("libopenmpt", "mptm", 64),
            ViewKind::FastTracker
        );
        assert_eq!(Views::select("libvgm", "vgm", 10), ViewKind::Vgm);
        assert_eq!(Views::select("LIBVGM", "VGM", 10), ViewKind::Vgm);
        assert_eq!(Views::select("v2m", "v2m", 16), ViewKind::V2m);
        assert_eq!(Views::select("V2M", "V2M", 16), ViewKind::V2m);
        assert_eq!(Views::select("sidplayfp", "sid", 0), ViewKind::Sid);
        assert_eq!(Views::select("SIDPLAYFP", "SID", 3), ViewKind::Sid);
        assert_eq!(Views::select("tfmx", "mdat", 8), ViewKind::Tfmx);
        assert_eq!(Views::select("TFMX", "TFMX", 4), ViewKind::Tfmx);
    }

    #[test]
    fn every_unported_slot_resolves_to_the_general_fallback() {
        let mut views = Views::with_view(ViewKind::General, Box::new(DummyView));
        let Some(general) = views
            .get_mut(ViewKind::General)
            .map(|view| view as *mut dyn View as *mut ())
        else {
            panic!("general view must be installed");
        };
        for kind in ALL_VIEW_KINDS {
            let Some(selected) = views
                .get_mut(kind)
                .map(|view| view as *mut dyn View as *mut ())
            else {
                panic!("every slot must resolve");
            };
            assert_eq!(selected, general, "wrong fallback for {kind:?}");
        }
    }

    #[test]
    fn seven_view_construction_installs_the_v2m_slot() {
        let mut views =
            Views::from_views(ALL_VIEW_KINDS.map(|_| Box::new(DummyView) as Box<dyn View>));
        assert!(views.get_mut(ViewKind::V2m).is_some());
    }
}
