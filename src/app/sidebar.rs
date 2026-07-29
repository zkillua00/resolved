use super::*;

impl ApiTester {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.sidebar_tab {
            SidebarTab::Collections => self.render_collections(cx),
            SidebarTab::Environments => self.render_environment_browser(cx),
            SidebarTab::History => self.render_history(cx),
        }
    }
}
