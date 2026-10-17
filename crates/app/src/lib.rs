pub mod app;
pub mod folders;
pub mod html;
pub mod keys;
pub mod list;
pub mod palette;
pub mod preferences;
pub mod sidebar;
pub mod snooze;
pub mod theme;
pub mod thread;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Archive,
        Clock,
        Inbox,
        FileText,
        Send,
        ShieldAlert,
        Trash,
        Mails,
        FolderInput,
        Paperclip,
        Settings,
        ChevronsUpDown
    ]
);

/// Component icons plus the two toolbar icons outside the default bundle.
pub struct Assets;

impl gpui_kit::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
