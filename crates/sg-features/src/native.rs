//! Native ntdll layouts + pure algorithms (no system calls) — ported from ViVe
//! (GPL-3.0, @thebookisclosed): `NativeStructs.cs`, `ObfuscationHelpers.cs`, `FeatureManager.cs`
//! (`SetFeatureConfigurationsInRegistry`), `ViVeTool/Program.cs` (`MakePriorityFixUpdates`).
//!
//! Everything here runs on every platform so it can be checked with pure tests; the real calls live in `platform`.

use crate::model::{
    EnabledState, FeatureConfig, MAX_VARIANT, Operation, PayloadKind, Priority, Subscription,
    Update, UsageReport,
};

/// `STATUS_INVALID_PARAMETER` — returned when the update data violates ViVe's rules (no system call is made).
pub const STATUS_INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;

/// `RTL_FEATURE_CONFIGURATION` (12 bytes): ID, bit-packed `CompactState`, variant payload.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtlFeatureConfiguration {
    pub feature_id: u32,
    pub compact_state: u32,
    pub variant_payload: u32,
}

impl RtlFeatureConfiguration {
    // Bit layout of CompactState (exactly as in NativeStructs.cs):
    //   0..3 Priority | 4..5 EnabledState | 6 IsWexp | 7 HasSubscriptions | 8..13 Variant | 14..15 PayloadKind
    pub fn priority(&self) -> u32 {
        self.compact_state & 0xF
    }
    pub fn set_priority(&mut self, v: u32) {
        self.compact_state = (self.compact_state & !0xF) | (v & 0xF);
    }
    pub fn enabled_state(&self) -> u32 {
        (self.compact_state & 0x30) >> 4
    }
    pub fn set_enabled_state(&mut self, v: u32) {
        self.compact_state = (self.compact_state & !0x30) | ((v & 0x3) << 4);
    }
    pub fn is_wexp(&self) -> bool {
        self.compact_state & 0x40 != 0
    }
    pub fn set_is_wexp(&mut self, v: bool) {
        self.compact_state = (self.compact_state & !0x40) | (u32::from(v) << 6);
    }
    pub fn has_subscriptions(&self) -> bool {
        self.compact_state & 0x80 != 0
    }
    pub fn set_has_subscriptions(&mut self, v: bool) {
        self.compact_state = (self.compact_state & !0x80) | (u32::from(v) << 7);
    }
    pub fn variant(&self) -> u32 {
        (self.compact_state & 0x3F00) >> 8
    }
    pub fn set_variant(&mut self, v: u32) {
        self.compact_state = (self.compact_state & !0x3F00) | ((v & 0x3F) << 8);
    }
    pub fn payload_kind(&self) -> u32 {
        (self.compact_state & 0xC000) >> 14
    }
    pub fn set_payload_kind(&mut self, v: u32) {
        self.compact_state = (self.compact_state & !0xC000) | ((v & 0x3) << 14);
    }

    /// Converts to the common type; an unknown priority gives `None`, an unknown state/payload falls back to the default.
    pub fn to_config(&self) -> Option<FeatureConfig> {
        Some(FeatureConfig {
            id: self.feature_id,
            priority: Priority::from_u32(self.priority())?,
            state: EnabledState::from_u32(self.enabled_state()).unwrap_or(EnabledState::Default),
            variant: self.variant() as u8,
            variant_payload_kind: PayloadKind::from_u32(self.payload_kind())
                .unwrap_or(PayloadKind::None),
            variant_payload: self.variant_payload,
            wexp: self.is_wexp(),
        })
    }
}

/// `RTL_FEATURE_CONFIGURATION_UPDATE` (32 bytes, eight consecutive u32 fields).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtlFeatureConfigurationUpdate {
    pub feature_id: u32,
    pub priority: u32,
    pub enabled_state: u32,
    pub enabled_state_options: u32,
    pub variant: u32,
    pub variant_payload_kind: u32,
    pub variant_payload: u32,
    pub operation: u32,
}

impl RtlFeatureConfigurationUpdate {
    pub fn from_update(u: &Update) -> Self {
        Self {
            feature_id: u.id,
            priority: u.priority.as_u32(),
            enabled_state: u.state.as_u32(),
            enabled_state_options: u.state_options,
            variant: u32::from(u.variant),
            variant_payload_kind: u.variant_payload_kind.as_u32(),
            variant_payload: u.variant_payload,
            operation: u.operation.0,
        }
    }
}

/// `RTL_FEATURE_USAGE_REPORT` (8 byte).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtlFeatureUsageReport {
    pub feature_id: u32,
    pub reporting_kind: u16,
    pub reporting_options: u16,
}

impl From<&UsageReport> for RtlFeatureUsageReport {
    fn from(r: &UsageReport) -> Self {
        Self {
            feature_id: r.id,
            reporting_kind: r.reporting_kind,
            reporting_options: r.reporting_options,
        }
    }
}

/// `RTL_FEATURE_USAGE_SUBSCRIPTION_DETAILS` (16 bytes, `ReportingTarget` aligned to 8).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RtlFeatureUsageSubscriptionDetails {
    pub feature_id: u32,
    pub reporting_kind: u16,
    pub reporting_options: u16,
    pub reporting_target: u64,
}

impl From<&Subscription> for RtlFeatureUsageSubscriptionDetails {
    fn from(s: &Subscription) -> Self {
        Self {
            feature_id: s.id,
            reporting_kind: s.reporting_kind,
            reporting_options: s.reporting_options,
            reporting_target: s.reporting_target,
        }
    }
}

impl From<&RtlFeatureUsageSubscriptionDetails> for Subscription {
    fn from(s: &RtlFeatureUsageSubscriptionDetails) -> Self {
        Self {
            id: s.feature_id,
            reporting_kind: s.reporting_kind,
            reporting_options: s.reporting_options,
            reporting_target: s.reporting_target,
        }
    }
}

fn swap_bytes_vive(x: u32) -> u32 {
    let x = x.rotate_left(16);
    ((x & 0xFF00_FF00) >> 8) | ((x & 0x00FF_00FF) << 8)
}

/// `ObfuscationHelpers.ObfuscateFeatureId`. C#'s `RotateRight32(v, -1)` (the shift count is masked to 5 bits)
/// is a right rotation by 31 bits = a left rotation by 1 bit.
pub fn obfuscate_feature_id(id: u32) -> u32 {
    (swap_bytes_vive(id ^ 0x7416_1A4E) ^ 0x8FB2_3D4F).rotate_left(1) ^ 0x833E_A8FF
}

/// `ObfuscationHelpers.DeobfuscateFeatureId`.
pub fn deobfuscate_feature_id(id: u32) -> u32 {
    swap_bytes_vive((id ^ 0x833E_A8FF).rotate_right(1) ^ 0x8FB2_3D4F) ^ 0x7416_1A4E
}

/// Rules of `FeatureManager.SetFeatureConfigurations` + the setters in `NativeStructs.cs`:
/// immutable priority, `UserPolicy` with attributes other than the state, variant > 63, options > 1, operation > 4.
pub fn validate_update(u: &Update) -> Result<(), i32> {
    if u.priority.is_immutable() {
        return Err(STATUS_INVALID_PARAMETER);
    }
    if u.priority == Priority::UserPolicy && !u.user_policy_compatible() {
        return Err(STATUS_INVALID_PARAMETER);
    }
    if u.variant > MAX_VARIANT || u.state_options > 1 || u.operation.0 > 4 {
        return Err(STATUS_INVALID_PARAMETER);
    }
    Ok(())
}

const OVERRIDES: &str = r"SYSTEM\CurrentControlSet\Control\FeatureManagement\Overrides";
const POLICY_OVERRIDES: &str =
    r"SYSTEM\CurrentControlSet\Policies\Microsoft\FeatureManagement\Overrides";
const USAGE_SUBSCRIPTIONS: &str =
    r"SYSTEM\CurrentControlSet\Control\FeatureManagement\UsageSubscriptions";
/// A registry (HKLM) operation that `SetFeatureConfigurationsInRegistry` will perform — computed up front for testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegOp {
    /// `CreateSubKey` (create if missing).
    EnsureKey { key: String },
    /// `DeleteSubKeyTree(key, false)` — skipped if the key does not exist.
    DeleteTree { key: String },
    /// Open the key (if it exists) and then `DeleteValue(name)`.
    DeleteValueIfKeyExists { key: String, name: String },
    SetDword {
        key: String,
        name: String,
        value: u32,
    },
}

/// The registry operations for one update, in ViVe's exact order (Boot store).
pub fn registry_ops(u: &Update) -> Vec<RegOp> {
    let obf = obfuscate_feature_id(u.id).to_string();
    let user_policy = u.priority == Priority::UserPolicy;
    let key = if user_policy {
        POLICY_OVERRIDES.to_string()
    } else {
        format!(r"{OVERRIDES}\{}\{obf}", u.priority.as_u32())
    };
    // ViVe compares `Operation == ResetState` (equality, not a flag test).
    if u.operation == Operation::RESET_STATE {
        return if user_policy {
            vec![RegOp::DeleteValueIfKeyExists { key, name: obf }]
        } else {
            vec![RegOp::DeleteTree { key }]
        };
    }
    let mut ops = vec![RegOp::EnsureKey { key: key.clone() }];
    let set = |name: &str, value: u32| RegOp::SetDword {
        key: key.clone(),
        name: name.to_string(),
        value,
    };
    if u.operation.contains(Operation::FEATURE_STATE) {
        if user_policy {
            ops.push(RegOp::SetDword {
                key: key.clone(),
                name: obf,
                value: u.state.as_u32(),
            });
        } else {
            ops.push(set("EnabledState", u.state.as_u32()));
            ops.push(set("EnabledStateOptions", u.state_options));
        }
    }
    if !user_policy && u.operation.contains(Operation::VARIANT_STATE) {
        ops.push(set("Variant", u32::from(u.variant)));
        ops.push(set("VariantPayload", u.variant_payload));
        ops.push(set("VariantPayloadKind", u.variant_payload_kind.as_u32()));
    }
    ops
}

/// The `UsageSubscriptions\<obfuscated id>` key (excluding the child GUID).
pub fn subscription_key(id: u32) -> String {
    format!(r"{USAGE_SUBSCRIPTIONS}\{}", obfuscate_feature_id(id))
}

/// ViVeTool's `MakePriorityFixUpdates` (`/fixpriority`): for every `Service`-priority configuration that is not
/// Wexp, delete it at `Service` and set it again identically at the `User` priority.
pub fn priority_fix_updates(configs: &[RtlFeatureConfiguration]) -> Vec<Update> {
    let service = Priority::Service.as_u32();
    let mut out = Vec::new();
    for c in configs
        .iter()
        .filter(|c| c.priority() == service && !c.is_wexp())
    {
        out.push(Update::reset(c.feature_id, Priority::Service));
        out.push(Update {
            id: c.feature_id,
            priority: Priority::User,
            state: EnabledState::from_u32(c.enabled_state()).unwrap_or(EnabledState::Default),
            state_options: 0,
            variant: c.variant() as u8,
            variant_payload_kind: PayloadKind::from_u32(c.payload_kind())
                .unwrap_or(PayloadKind::None),
            variant_payload: c.variant_payload,
            operation: Operation::FEATURE_STATE | Operation::VARIANT_STATE,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn struct_sizes_match_vive() {
        // RTL_FEATURE_CONFIGURATION: uint FeatureId(0), uint CompactState(4), uint VariantPayload(8) => 12.
        assert_eq!(size_of::<RtlFeatureConfiguration>(), 12);
        assert_eq!(offset_of!(RtlFeatureConfiguration, feature_id), 0);
        assert_eq!(offset_of!(RtlFeatureConfiguration, compact_state), 4);
        assert_eq!(offset_of!(RtlFeatureConfiguration, variant_payload), 8);
        // RTL_FEATURE_CONFIGURATION_UPDATE: 8 consecutive 4-byte fields => 32.
        assert_eq!(size_of::<RtlFeatureConfigurationUpdate>(), 32);
        assert_eq!(offset_of!(RtlFeatureConfigurationUpdate, feature_id), 0);
        assert_eq!(offset_of!(RtlFeatureConfigurationUpdate, priority), 4);
        assert_eq!(offset_of!(RtlFeatureConfigurationUpdate, enabled_state), 8);
        assert_eq!(
            offset_of!(RtlFeatureConfigurationUpdate, enabled_state_options),
            12
        );
        assert_eq!(offset_of!(RtlFeatureConfigurationUpdate, variant), 16);
        assert_eq!(
            offset_of!(RtlFeatureConfigurationUpdate, variant_payload_kind),
            20
        );
        assert_eq!(
            offset_of!(RtlFeatureConfigurationUpdate, variant_payload),
            24
        );
        assert_eq!(offset_of!(RtlFeatureConfigurationUpdate, operation), 28);
        // RTL_FEATURE_USAGE_REPORT: uint(0) ushort(4) ushort(6) => 8.
        assert_eq!(size_of::<RtlFeatureUsageReport>(), 8);
        assert_eq!(offset_of!(RtlFeatureUsageReport, feature_id), 0);
        assert_eq!(offset_of!(RtlFeatureUsageReport, reporting_kind), 4);
        assert_eq!(offset_of!(RtlFeatureUsageReport, reporting_options), 6);
        // RTL_FEATURE_USAGE_SUBSCRIPTION_DETAILS: uint(0) ushort(4) ushort(6) ulong(8, aligned to 8) => 16.
        assert_eq!(size_of::<RtlFeatureUsageSubscriptionDetails>(), 16);
        assert_eq!(
            offset_of!(RtlFeatureUsageSubscriptionDetails, feature_id),
            0
        );
        assert_eq!(
            offset_of!(RtlFeatureUsageSubscriptionDetails, reporting_kind),
            4
        );
        assert_eq!(
            offset_of!(RtlFeatureUsageSubscriptionDetails, reporting_options),
            6
        );
        assert_eq!(
            offset_of!(RtlFeatureUsageSubscriptionDetails, reporting_target),
            8
        );
    }

    #[test]
    fn bitfield_pack_roundtrip() {
        let mut c = RtlFeatureConfiguration::default();
        c.set_priority(15);
        c.set_enabled_state(2);
        c.set_is_wexp(true);
        c.set_has_subscriptions(true);
        c.set_variant(63);
        c.set_payload_kind(3);
        // 0xF | 2<<4 | 1<<6 | 1<<7 | 63<<8 | 3<<14
        assert_eq!(c.compact_state, 0xF | 0x20 | 0x40 | 0x80 | 0x3F00 | 0xC000);
        assert_eq!(c.priority(), 15);
        assert_eq!(c.enabled_state(), 2);
        assert!(c.is_wexp() && c.has_subscriptions());
        assert_eq!(c.variant(), 63);
        assert_eq!(c.payload_kind(), 3);
        // Changing one field does not corrupt the others.
        c.set_variant(5);
        c.set_enabled_state(1);
        c.set_is_wexp(false);
        assert_eq!(c.compact_state, 0xF | 0x10 | 0x80 | (5 << 8) | 0xC000);
        // Real value: Priority=User(8), Enabled(2), variant 1 => 0x128.
        let r = RtlFeatureConfiguration {
            feature_id: 1,
            compact_state: 0x128,
            variant_payload: 0,
        };
        assert_eq!((r.priority(), r.enabled_state(), r.variant()), (8, 2, 1));
    }

    #[test]
    fn obfuscation_matches_vive() {
        // Computed by hand from ObfuscationHelpers.cs (C#'s RotateRight32(x, -1) = rotate right 31 = rotate left 1):
        // id 0: 0x74161A4E -> SwapBytes 0x4E1A1674 -> ^0x8FB23D4F = 0xC1A82B3B -> rotate left 1 = 0x83505677
        //       -> ^0x833EA8FF = 0x006EFE88 = 7274120.
        assert_eq!(obfuscate_feature_id(0), 7_274_120);
        // id 0xFFFFFFFF: ^0x74161A4E = 0x8BE9E5B1 -> 0xB1E5E98B -> ^0x8FB23D4F = 0x3E57D4C4
        //       -> rotate left 1 = 0x7CAFA988 -> ^0x833EA8FF = 0xFF910177 = 4287693175.
        assert_eq!(obfuscate_feature_id(0xFFFF_FFFF), 4_287_693_175);
        assert_eq!(deobfuscate_feature_id(7_274_120), 0);
        assert_eq!(deobfuscate_feature_id(4_287_693_175), 0xFFFF_FFFF);
        for id in [1u32, 26008830, 0x1234_5678, 0x8000_0000, 42304559] {
            assert_eq!(deobfuscate_feature_id(obfuscate_feature_id(id)), id);
        }
    }

    fn upd(id: u32, p: Priority, op: Operation) -> Update {
        Update {
            id,
            priority: p,
            state: EnabledState::Enabled,
            state_options: 1,
            variant: 3,
            variant_payload_kind: PayloadKind::Resident,
            variant_payload: 9,
            operation: op,
        }
    }

    #[test]
    fn update_maps_to_native() {
        let n = RtlFeatureConfigurationUpdate::from_update(&upd(
            7,
            Priority::User,
            Operation::FEATURE_STATE | Operation::VARIANT_STATE,
        ));
        assert_eq!(
            (
                n.feature_id,
                n.priority,
                n.enabled_state,
                n.enabled_state_options,
                n.variant,
                n.variant_payload_kind,
                n.variant_payload,
                n.operation
            ),
            (7, 8, 2, 1, 3, 1, 9, 3)
        );
    }

    #[test]
    fn validate_rejects_like_vive() {
        let ok = upd(1, Priority::User, Operation::FEATURE_STATE);
        assert!(validate_update(&ok).is_ok());
        for p in [
            Priority::ImageDefault,
            Priority::Ekb,
            Priority::ImageDefaultEditionOverride,
            Priority::Security,
            Priority::ImageOverride,
        ] {
            assert!(validate_update(&upd(1, p, Operation::FEATURE_STATE)).is_err());
        }
        // UserPolicy only accepts EnabledState.
        assert!(validate_update(&upd(1, Priority::UserPolicy, Operation::FEATURE_STATE)).is_err());
        let mut up = upd(1, Priority::UserPolicy, Operation::FEATURE_STATE);
        up.state_options = 0;
        up.variant = 0;
        up.variant_payload_kind = PayloadKind::None;
        up.variant_payload = 0;
        assert!(validate_update(&up).is_ok());
        let mut bad = ok;
        bad.variant = 64;
        assert!(validate_update(&bad).is_err());
        bad = ok;
        bad.state_options = 2;
        assert!(validate_update(&bad).is_err());
        bad = ok;
        bad.operation = Operation(5);
        assert!(validate_update(&bad).is_err());
    }

    #[test]
    fn registry_ops_follow_vive() {
        let obf = obfuscate_feature_id(0).to_string();
        assert_eq!(obf, "7274120");
        let key = format!(r"SYSTEM\CurrentControlSet\Control\FeatureManagement\Overrides\8\{obf}");
        // Set state + variant at the User priority.
        let ops = registry_ops(&upd(
            0,
            Priority::User,
            Operation::FEATURE_STATE | Operation::VARIANT_STATE,
        ));
        let dw = |name: &str, value: u32| RegOp::SetDword {
            key: key.clone(),
            name: name.into(),
            value,
        };
        assert_eq!(
            ops,
            vec![
                RegOp::EnsureKey { key: key.clone() },
                dw("EnabledState", 2),
                dw("EnabledStateOptions", 1),
                dw("Variant", 3),
                dw("VariantPayload", 9),
                dw("VariantPayloadKind", 1),
            ]
        );
        // FeatureState only: the variant is not written.
        let ops = registry_ops(&upd(0, Priority::Service, Operation::FEATURE_STATE));
        assert_eq!(ops.len(), 3);
        // ResetState: delete the whole key tree.
        let ops = registry_ops(&upd(0, Priority::User, Operation::RESET_STATE));
        assert_eq!(ops, vec![RegOp::DeleteTree { key }]);
        // UserPolicy: a separate policy key, value name = obfuscated ID, EnabledState only.
        let pol =
            r"SYSTEM\CurrentControlSet\Policies\Microsoft\FeatureManagement\Overrides".to_string();
        let mut u = upd(
            0,
            Priority::UserPolicy,
            Operation::FEATURE_STATE | Operation::VARIANT_STATE,
        );
        u.state_options = 0;
        assert_eq!(
            registry_ops(&u),
            vec![
                RegOp::EnsureKey { key: pol.clone() },
                RegOp::SetDword {
                    key: pol.clone(),
                    name: obf.clone(),
                    value: 2
                }
            ]
        );
        assert_eq!(
            registry_ops(&upd(0, Priority::UserPolicy, Operation::RESET_STATE)),
            vec![RegOp::DeleteValueIfKeyExists {
                key: pol,
                name: obf
            }]
        );
    }

    #[test]
    fn subscription_registry_names() {
        assert_eq!(
            subscription_key(0),
            r"SYSTEM\CurrentControlSet\Control\FeatureManagement\UsageSubscriptions\7274120"
        );
    }

    #[test]
    fn priority_fix_updates_follow_vive() {
        let mk = |id, prio, state, variant, kind, payload, wexp| {
            let mut c = RtlFeatureConfiguration {
                feature_id: id,
                compact_state: 0,
                variant_payload: payload,
            };
            c.set_priority(prio);
            c.set_enabled_state(state);
            c.set_variant(variant);
            c.set_payload_kind(kind);
            c.set_is_wexp(wexp);
            c
        };
        let cfgs = [
            mk(10, 4, 2, 1, 1, 7, false), // Service, not wexp => fix
            mk(11, 4, 2, 0, 0, 0, true),  // Service but wexp => skip
            mk(12, 8, 2, 0, 0, 0, false), // User => skip
        ];
        let ups = priority_fix_updates(&cfgs);
        assert_eq!(ups.len(), 2);
        assert_eq!(ups[0], Update::reset(10, Priority::Service));
        assert_eq!(ups[1].priority, Priority::User);
        assert_eq!(ups[1].id, 10);
        assert_eq!(ups[1].state, EnabledState::Enabled);
        assert_eq!(
            (
                ups[1].variant,
                ups[1].variant_payload_kind,
                ups[1].variant_payload
            ),
            (1, PayloadKind::Resident, 7)
        );
        assert_eq!(ups[1].state_options, 0);
        assert_eq!(
            ups[1].operation,
            Operation::FEATURE_STATE | Operation::VARIANT_STATE
        );
        assert!(priority_fix_updates(&cfgs[1..]).is_empty());
    }

    #[test]
    fn set_from_user_policy_only_state() {
        // Decision T2: restoring the UserPolicy priority writes only the state (ViVe does not store a variant at this priority).
        let cfg = FeatureConfig {
            id: 5,
            priority: Priority::UserPolicy,
            state: EnabledState::Disabled,
            variant: 2,
            variant_payload_kind: PayloadKind::Resident,
            variant_payload: 4,
            wexp: false,
        };
        let u = Update::set_from(&cfg);
        assert_eq!(u.operation, Operation::FEATURE_STATE);
        assert!(validate_update(&u).is_ok());
        let ops = registry_ops(&u);
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn wexp_flag_roundtrips_through_native() {
        let mut raw = RtlFeatureConfiguration { feature_id: 9, compact_state: 0, variant_payload: 0 };
        raw.set_priority(Priority::User.as_u32());
        raw.set_enabled_state(2);
        raw.set_is_wexp(true);
        let cfg = raw.to_config().unwrap();
        assert!(cfg.wexp);
        let up = Update::set_from(&cfg);
        assert_eq!(up.state_options, 1);
        assert_eq!(RtlFeatureConfigurationUpdate::from_update(&up).enabled_state_options, 1);
        raw.set_is_wexp(false);
        let cfg2 = raw.to_config().unwrap();
        assert!(!cfg2.wexp);
        assert_eq!(Update::set_from(&cfg2).state_options, 0);
    }
}
