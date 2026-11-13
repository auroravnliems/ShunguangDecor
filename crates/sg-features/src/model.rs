//! Data model — ported from ViVe (GPL-3.0, @thebookisclosed): `NativeEnums.cs`, `FeatureManager.cs`.

use serde::{Deserialize, Serialize};

/// The largest variant ViVe allows (6 bits).
pub const MAX_VARIANT: u8 = 63;

/// Configuration store: `Runtime` (lost on restart) or `Boot` (written to the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    Runtime,
    Boot,
}

/// `RTL_FEATURE_CONFIGURATION_PRIORITY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    ImageDefault,
    Ekb,
    Safeguard,
    ImageDefaultEditionOverride,
    Service,
    Dynamic,
    User,
    Security,
    UserPolicy,
    Test,
    ImageOverride,
}

impl Priority {
    /// The native numeric value.
    pub fn as_u32(self) -> u32 {
        match self {
            Self::ImageDefault => 0,
            Self::Ekb => 1,
            Self::Safeguard => 2,
            Self::ImageDefaultEditionOverride => 3,
            Self::Service => 4,
            Self::Dynamic => 6,
            Self::User => 8,
            Self::Security => 9,
            Self::UserPolicy => 10,
            Self::Test => 12,
            Self::ImageOverride => 15,
        }
    }

    /// Converts from the native number; an unknown number gives `None`.
    pub fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::ImageDefault,
            1 => Self::Ekb,
            2 => Self::Safeguard,
            3 => Self::ImageDefaultEditionOverride,
            4 => Self::Service,
            6 => Self::Dynamic,
            8 => Self::User,
            9 => Self::Security,
            10 => Self::UserPolicy,
            12 => Self::Test,
            15 => Self::ImageOverride,
            _ => return None,
        })
    }

    /// Priorities that must not be written (`FeatureManager.ImmutablePriorities`).
    pub fn is_immutable(self) -> bool {
        matches!(
            self,
            Self::ImageDefault
                | Self::Ekb
                | Self::ImageDefaultEditionOverride
                | Self::Security
                | Self::ImageOverride
        )
    }
}

/// `RTL_FEATURE_ENABLED_STATE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnabledState {
    Default,
    Disabled,
    Enabled,
}

impl EnabledState {
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Default => 0,
            Self::Disabled => 1,
            Self::Enabled => 2,
        }
    }

    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Default),
            1 => Some(Self::Disabled),
            2 => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// `RTL_FEATURE_VARIANT_PAYLOAD_KIND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    #[default]
    None,
    Resident,
    External,
}

impl PayloadKind {
    pub fn as_u32(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Resident => 1,
            Self::External => 2,
        }
    }

    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Resident),
            2 => Some(Self::External),
            _ => None,
        }
    }
}

/// An existing feature configuration (a query result / an export-import row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureConfig {
    pub id: u32,
    pub priority: Priority,
    pub state: EnabledState,
    /// 0..=63.
    #[serde(default)]
    pub variant: u8,
    #[serde(default)]
    pub variant_payload_kind: PayloadKind,
    #[serde(default)]
    pub variant_payload: u32,
    /// Experimental configuration (Wexp, `EnabledStateOptions = WexpConfig`); preserved on import/export/undo as ViVeTool does.
    #[serde(default)]
    pub wexp: bool,
}

/// Operation flags of `RTL_FEATURE_CONFIGURATION_OPERATION` (hand-written, to avoid adding a bitflags dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Operation(pub u32);

impl Operation {
    pub const NONE: Self = Self(0);
    pub const FEATURE_STATE: Self = Self(1);
    pub const VARIANT_STATE: Self = Self(2);
    /// Clears the configuration at that priority (ViVe `/reset` uses only this flag; ViVe has no "ResetVariant" flag).
    pub const RESET_STATE: Self = Self(4);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Operation {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// `RTL_FEATURE_CONFIGURATION_UPDATE` (the data-side form, not the native layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub id: u32,
    pub priority: Priority,
    pub state: EnabledState,
    /// `RTL_FEATURE_ENABLED_STATE_OPTIONS` (0 = None, 1 = WexpConfig).
    #[serde(default)]
    pub state_options: u32,
    #[serde(default)]
    pub variant: u8,
    #[serde(default)]
    pub variant_payload_kind: PayloadKind,
    #[serde(default)]
    pub variant_payload: u32,
    pub operation: Operation,
}

impl Update {
    /// An update that clears the configuration of `(id, priority)` — like `ViVeTool /reset`.
    pub fn reset(id: u32, priority: Priority) -> Self {
        Self {
            id,
            priority,
            state: EnabledState::Default,
            state_options: 0,
            variant: 0,
            variant_payload_kind: PayloadKind::None,
            variant_payload: 0,
            operation: Operation::RESET_STATE,
        }
    }

    /// An update that sets exactly `cfg` again (state + variant).
    ///
    /// Following ViVe (`RTL_FEATURE_CONFIGURATION_UPDATE.UserPolicyPriorityCompatible`): the `UserPolicy` priority
    /// can only store `EnabledState`, so only the `FEATURE_STATE` flag is set and the variant/payload are dropped — if the
    /// variant were sent as well, `platform::set` would refuse it just like ViVe.
    pub fn set_from(cfg: &FeatureConfig) -> Self {
        if cfg.priority == Priority::UserPolicy {
            return Self {
                id: cfg.id,
                priority: cfg.priority,
                state: cfg.state,
                state_options: 0,
                variant: 0,
                variant_payload_kind: PayloadKind::None,
                variant_payload: 0,
                operation: Operation::FEATURE_STATE,
            };
        }
        Self {
            id: cfg.id,
            priority: cfg.priority,
            state: cfg.state,
            state_options: u32::from(cfg.wexp),
            variant: cfg.variant,
            variant_payload_kind: cfg.variant_payload_kind,
            variant_payload: cfg.variant_payload,
            operation: Operation::FEATURE_STATE | Operation::VARIANT_STATE,
        }
    }

    /// ViVe's `UserPolicyPriorityCompatible`: no property other than `state` is set.
    pub fn user_policy_compatible(&self) -> bool {
        self.state_options == 0
            && self.variant == 0
            && self.variant_payload_kind == PayloadKind::None
            && self.variant_payload == 0
    }
}

/// `RTL_FEATURE_USAGE_SUBSCRIPTION_DETAILS` (the data-side form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: u32,
    pub reporting_kind: u16,
    #[serde(default)]
    pub reporting_options: u16,
    pub reporting_target: u64,
}

/// `RTL_FEATURE_USAGE_REPORT` (the data-side form). ViVe has no "count" field — only kind + options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    pub id: u32,
    pub reporting_kind: u16,
    #[serde(default)]
    pub reporting_options: u16,
}
