pub mod app;
pub mod ask;
pub mod compose;
pub mod contact;
pub mod folders;
pub mod html;
pub mod keys;
pub mod list;
pub mod palette;
pub mod preferences;
pub mod sidebar;
pub mod snooze;
pub mod tags;
pub mod theme;
pub mod thread;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Archive,
        Clock,
        Hourglass,
        CalendarClock,
        Inbox,
        FileText,
        FileImage,
        FileSpreadsheet,
        FileArchive,
        FileMusic,
        FilePlay,
        Send,
        ShieldAlert,
        Trash,
        Mails,
        FolderInput,
        Paperclip,
        Settings,
        ChevronsUpDown,
        Reply,
        ReplyAll,
        Forward,
        Keyboard,
        MailOpen,
        X,
        SquarePen,
        RefreshCw,
        Sparkles,
        Lock,
        Plus,
        Tag,
        Maximize2,
        Minimize2,
        TextQuote
    ]
);

/// The provider marks the combined inbox paints, hand-drawn rather than Lucide's set because
/// they name a mail service, not an action. SVG bytes are borrowed from the binary, so a row
/// costs no allocation to draw one.
const PROVIDER_MARKS: [(&str, &[u8]); 2] = [
    (
        "providers/proton.svg",
        include_bytes!("../assets/providers/proton.svg"),
    ),
    (
        "providers/outlook.svg",
        include_bytes!("../assets/providers/outlook.svg"),
    ),
];

/// Component icons plus the ones Kite uses from outside the default bundle (an icon that is in
/// neither renders blank; `tests/icons.rs` checks every one Kite names).
pub struct Assets;

impl gpui_kit::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = PROVIDER_MARKS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(std::borrow::Cow::Borrowed(bytes)));
        }
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(
            PROVIDER_MARKS
                .iter()
                .map(|(name, _)| *name)
                .filter(|name| name.starts_with(path))
                .map(gpui_kit::SharedString::from),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
