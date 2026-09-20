//! Spiral Robotics interface primitives.
//!
//! One design language, spelled once: every Studio surface builds from the
//! same actions, fields, facts, disclosures, trees, reorder gestures, and
//! notices, and the surface owns only layout and domain content. The
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
    Disclosure, DisclosureProps, Reorder, ReorderHandle, ReorderHandleProps, ReorderList,
    ReorderListProps, ReorderMove, SheetHeading, SheetPosition, SideSheet, SideSheetProps, Tree,
    TreeBranch, TreeItem, TreeItemProps, TreeProps,
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
    Chip, ChipProps, ChipTone, EmptyState, EmptyStateProps, Fact, FactLayout, FactList,
    FactListProps, FactProps, Notice, NoticeProps, NoticeTone,
};
pub use foundation::{
    ArrowDownIcon, ArrowUpIcon, BrowserFocus, ChevronIcon, ChevronIconProps, CloseIcon, DeleteIcon,
    GripIcon, RefreshIcon, UiStyles, UnassignIcon,
};

/// Everything a surface typically needs.
pub mod prelude {
    pub use crate::{
        ActionPurpose, BlockingConfirmation, BrowserFocus, Button, ButtonAuxiliaryHandlers,
        ButtonSize, ButtonVariant, ChevronIcon, Chip, ChipTone, CloseIcon, ConfirmationCopy,
        ConfirmationDismissal, ConfirmationResponse, DeleteIcon, Disclosure, EmptyState, Fact,
        FactLayout, FactList, Field, FieldLayout, FileButton, FileInput, InlineConfirmation,
        LinkButton, ModeButton, ModeGroup, Notice, NoticeTone, RefreshIcon, Reorder, ReorderHandle,
        ReorderList, ReorderMove, Select, SheetHeading, SheetPosition, SideSheet, TextInput,
        TextInputKind, Toggle, Tree, TreeBranch, TreeItem, UnassignIcon,
    };
}
