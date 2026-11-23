//! Interactive hit regions for mouse hover, click, right-click rename, and wheel routing.

use crate::config::DockPosition;
use crate::model::{JobId, Language, ProcessKey, ProjectId};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

#[derive(Debug, Clone, PartialEq)]
pub enum HitAction {
    ExpandPanel,
    CollapsePanel,
    ToggleSticky,
    ToggleShowAllProcesses,
    OpenSettings,
    BackToExpanded,
    FocusSearch,
    ClearSearch,
    OpenProjectDetail(ProjectId),
    OpenJobDetail(ProjectId, JobId),
    OpenProjectFolder(ProjectId),
    OpenConfigDir,
    TogglePinProject(ProjectId),
    CycleSortMode,
    MoveProjectOrder(ProjectId, i32),
    MoveJobOrder(String, i32),
    OpenPort { port: u16, url: Option<String> },
    CopyText(String),
    HideProject(ProjectId),
    RestoreHiddenProjects,
    // Job & custom job actions
    RequestKillJob(JobId),
    ConfirmKillJob(JobId),
    RestartJob(JobId),
    StartCustomJob(JobId),
    RemoveWatchedJob(String),
    BeginRenameJob {
        match_hint: String,
        current_title: String,
    },
    BeginRenameProject {
        project_id: ProjectId,
        current_name: String,
    },
    SaveRename,
    CancelRename,
    BeginAddJob,
    FocusAddJobField(u8),
    SaveAddJob,
    CancelAddJob,
    WatchProcessFromAll {
        match_substr: String,
        title: String,
        working_dir: Option<String>,
    },
    CopyTerminalOutput,
    RefreshTerminalOutput,
    JumpTerminalLive,
    // Project & process kill/restart
    RequestKillProject(ProjectId),
    ConfirmKillProject(ProjectId),
    CancelKillConfirm,
    RequestKillProcess(ProcessKey),
    ConfirmKillProcess(ProcessKey),
    RestartProject(ProjectId),
    DismissAlert(u64),
    RestartAlertProject(ProjectId, u64),
    // Settings actions
    SetLanguage(Language),
    SetDockPosition(DockPosition),
    CycleMonitor,
    ToggleAlwaysOnTop,
    ToggleAcrylic,
    AdjustIdleOpacity(f32),
    AdjustOpacity(f32),
    ToggleFullscreenHide,
    ToggleCrashNotify,
    ToggleHotkeyPreset,
    ToggleAutostart,
    AdjustPeekDelay(i32),
}

#[derive(Debug, Clone)]
pub struct HitRegion {
    pub id: u32,
    pub rect: D2D_RECT_F,
    pub action: HitAction,
    pub right_click_action: Option<HitAction>,
}

#[derive(Debug, Default)]
pub struct HitMap {
    pub regions: Vec<HitRegion>,
    pub scroll_viewport: Option<D2D_RECT_F>,
    pub term_viewport: Option<D2D_RECT_F>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
        self.scroll_viewport = None;
        self.term_viewport = None;
    }

    pub fn push(&mut self, id: u32, rect: D2D_RECT_F, action: HitAction) {
        self.regions.push(HitRegion {
            id,
            rect,
            action,
            right_click_action: None,
        });
    }

    pub fn push_with_rclick(
        &mut self,
        id: u32,
        rect: D2D_RECT_F,
        action: HitAction,
        right_click_action: HitAction,
    ) {
        self.regions.push(HitRegion {
            id,
            rect,
            action,
            right_click_action: Some(right_click_action),
        });
    }

    pub fn push_clipped(
        &mut self,
        id: u32,
        mut rect: D2D_RECT_F,
        min_y: f32,
        max_y: f32,
        action: HitAction,
    ) {
        rect.top = rect.top.max(min_y);
        rect.bottom = rect.bottom.min(max_y);
        if rect.bottom > rect.top {
            self.push(id, rect, action);
        }
    }

    pub fn push_with_rclick_clipped(
        &mut self,
        id: u32,
        mut rect: D2D_RECT_F,
        min_y: f32,
        max_y: f32,
        action: HitAction,
        right_click_action: HitAction,
    ) {
        rect.top = rect.top.max(min_y);
        rect.bottom = rect.bottom.min(max_y);
        if rect.bottom > rect.top {
            self.push_with_rclick(id, rect, action, right_click_action);
        }
    }

    /// Applies a viewport translation + uniform scale (e.g. during panel morph or tab slide).
    pub fn transform(&mut self, off_x: f32, off_y: f32, scale: f32) {
        let map_rect = |r: &mut D2D_RECT_F| {
            r.left = off_x + r.left * scale;
            r.top = off_y + r.top * scale;
            r.right = off_x + r.right * scale;
            r.bottom = off_y + r.bottom * scale;
        };
        for region in &mut self.regions {
            map_rect(&mut region.rect);
        }
        if let Some(vp) = &mut self.scroll_viewport {
            map_rect(vp);
        }
        if let Some(tvp) = &mut self.term_viewport {
            map_rect(tvp);
        }
    }

    /// Topmost matching region (last pushed wins for overlapping buttons inside rows).
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&HitRegion> {
        self.regions.iter().rev().find(|r| {
            x >= r.rect.left && x <= r.rect.right && y >= r.rect.top && y <= r.rect.bottom
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: f32, t: f32, ri: f32, b: f32) -> D2D_RECT_F {
        D2D_RECT_F {
            left: l,
            top: t,
            right: ri,
            bottom: b,
        }
    }

    #[test]
    fn push_clipped_drops_offscreen_and_clamps_partial_overlap() {
        let mut hm = HitMap::default();
        hm.push_clipped(1, r(0.0, 10.0, 50.0, 40.0), 50.0, 200.0, HitAction::ExpandPanel);
        assert!(hm.regions.is_empty());

        hm.push_clipped(2, r(0.0, 40.0, 50.0, 90.0), 50.0, 200.0, HitAction::ExpandPanel);
        assert_eq!(hm.regions.len(), 1);
        assert_eq!(hm.regions[0].rect.top, 50.0);
        assert_eq!(hm.regions[0].rect.bottom, 90.0);
    }

    #[test]
    fn hit_test_prefers_topmost_and_transform_scales_viewports() {
        let mut hm = HitMap::default();
        hm.push(1, r(10.0, 10.0, 100.0, 40.0), HitAction::ExpandPanel);
        hm.push(2, r(60.0, 15.0, 90.0, 35.0), HitAction::OpenSettings);
        hm.scroll_viewport = Some(r(0.0, 50.0, 100.0, 200.0));

        assert_eq!(hm.hit_test(75.0, 25.0).map(|h| h.id), Some(2));
        assert_eq!(hm.hit_test(20.0, 25.0).map(|h| h.id), Some(1));

        hm.transform(20.0, 0.0, 0.5);
        assert_eq!(hm.hit_test(57.5, 12.5).map(|h| h.id), Some(2));
        assert_eq!(hm.scroll_viewport.unwrap().left, 20.0);
        assert_eq!(hm.scroll_viewport.unwrap().bottom, 100.0);
    }
}
