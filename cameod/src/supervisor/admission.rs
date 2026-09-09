use std::collections::HashSet;
use std::time::SystemTime;

/// A currently-resident endpoint as the admission decision sees it.
pub(super) struct ResidentVram {
    pub(super) id: String,
    pub(super) vram_bytes: u64,
    pub(super) last_used: SystemTime,
}

#[derive(Debug, PartialEq)]
pub(super) enum Admission {
    Admit,
    Evict(Vec<String>),
    Refuse,
}

/// Pure VRAM admission policy: refuse an impossible load, otherwise evict the
/// least-recently-used unprotected residents until the request fits.
pub(super) fn admit(budget: u64, need: u64, residents: &mut [ResidentVram]) -> Admission {
    if need > budget {
        return Admission::Refuse;
    }
    let used = residents
        .iter()
        .map(|resident| resident.vram_bytes)
        .fold(0_u64, u64::saturating_add);
    if used.saturating_add(need) <= budget {
        return Admission::Admit;
    }
    residents.sort_by_key(|resident| resident.last_used);
    let mut remaining = used;
    let mut evict = Vec::new();
    for resident in residents {
        remaining = remaining.saturating_sub(resident.vram_bytes);
        evict.push(resident.id.clone());
        if remaining.saturating_add(need) <= budget {
            return Admission::Evict(evict);
        }
    }
    Admission::Refuse
}

pub(super) fn exclude_leased_residents(
    residents: &mut Vec<ResidentVram>,
    protected: &HashSet<&str>,
) {
    residents.retain(|resident| !protected.contains(resident.id.as_str()));
}
