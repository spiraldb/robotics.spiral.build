//! Spiral Robotics interface primitives.
//!
//! One design language, spelled once: every Studio surface builds from the
//! same actions, fields, facts, disclosures, trees, and notices, and the
//! surface owns only layout and domain content. The
//! stylesheet ships with the crate; install it once with [`UiStyles`] beside
//! the application shell. Colours are role tokens (`--ui-*`) that follow the
//! host's `--tone-*` palette when it defines one.
//!
//! A [`Button`] names what it does through an [`ActionPurpose`] before it
//! chooses a look; the verb, not the variant, decides its destructive
//! treatment.

// Dioxus expands RSX interpolation such as `"{label}"` through `format!`, so
// this lint reports the macro's generated code rather than an avoidable call.
#![allow(clippy::useless_format)]

mod containers;
mod controls;
mod display;
mod foundation;

pub use containers::{
    Disclosure, DisclosureProps, Menu, MenuProps, Tree, TreeBranch, TreeItem, TreeItemProps,
    TreeProps,
};
pub use controls::{
    ActionPurpose, BlockingConfirmation, BlockingConfirmationProps, Button,
    ButtonAuxiliaryHandlers, ButtonProps, ButtonSize, ButtonVariant, ConfirmationCopy,
    ConfirmationDismissal, ConfirmationResponse, Field, FieldLayout, FieldProps, FileButton,
    FileButtonProps, FileInput, InlineConfirmation, InlineConfirmationProps, LinkButton,
    LinkButtonProps, ModeButton, ModeButtonProps, ModeGroup, ModeGroupProps, Select, SelectProps,
    TextInput, TextInputKind, TextInputProps, Toggle, ToggleProps,
};
pub use display::{
    EmptyState, EmptyStateProps, Fact, FactLayout, FactList, FactListProps, FactProps, KindMark,
    KindMarkProps, Notice, NoticeProps, NoticeTone,
};
pub use foundation::{
    BrowserFocus, CheckIcon, ChevronIcon, ChevronIconProps, CloseIcon, DeleteIcon, Flight, Motion,
    PauseIcon, Place, PlayIcon, RefreshIcon, Remount, SkipToEndIcon, SkipToStartIcon, SpinnerIcon,
    StepBackIcon, StepForwardIcon, UiStyles, UnassignIcon, ZoomInIcon, ZoomOutIcon,
};
