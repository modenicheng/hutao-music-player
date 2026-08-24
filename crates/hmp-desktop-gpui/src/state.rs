#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Library,
    Recommend,
    Search,
    Queue,
    Lyrics,
    Settings,
}

impl Page {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Library => "媒体库",
            Self::Recommend => "推荐",
            Self::Search => "搜索",
            Self::Queue => "播放队列",
            Self::Lyrics => "歌词",
            Self::Settings => "设置",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NavigationState {
    pub page: Page,
}

impl NavigationState {
    pub fn navigate(&mut self, page: Page) {
        self.page = page;
    }
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            page: Page::Library,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_starts_in_library_and_can_switch_pages() {
        let mut navigation = NavigationState::default();
        assert_eq!(navigation.page, Page::Library);

        navigation.navigate(Page::Lyrics);
        assert_eq!(navigation.page, Page::Lyrics);
    }

    #[test]
    fn every_sidebar_page_has_stable_copy() {
        let expected = [
            (Page::Search, "搜索"),
            (Page::Recommend, "推荐"),
            (Page::Library, "媒体库"),
            (Page::Queue, "播放队列"),
            (Page::Lyrics, "歌词"),
            (Page::Settings, "设置"),
        ];

        for (page, label) in expected {
            assert_eq!(page.label(), label);
        }
    }
}
