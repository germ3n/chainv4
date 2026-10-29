//! Whether the device is on its charger, for an app that only works hard on one, such as the
//! wallet mining on a phone. Android says it through its sticky ACTION_BATTERY_CHANGED broadcast,
//! iOS through UIDevice's battery state; each asks again every few seconds while an app wants to
//! know, and keeps what it found in the egui context. Elsewhere it is unknown.

/// How long an app's asking keeps the platform looking, in seconds: it asks each frame it cares.
#[cfg(any(target_os = "android", target_os = "ios", test))]
const WANTED_FOR: f64 = 10.0;
/// How often the platform looks again while wanted, in seconds.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) const LOOK_EVERY: f64 = 5.0;

fn found_id() -> egui::Id {
    egui::Id::unique("chainv4-ui charging")
}

fn wanted_id() -> egui::Id {
    egui::Id::unique("chainv4-ui charging wanted")
}

/// Whether the device is on its charger, charging or full: Some(false) on battery, None where it
/// is not known, before the platform first looked, or off Android and iOS. Asking keeps the
/// platform looking, every few seconds, for a while.
pub fn charging(ctx: &egui::Context) -> Option<bool> {
    let time = ctx.input(|input| input.time);
    ctx.data_mut(|data| data.insert_temp(wanted_id(), time));

    ctx.data(|data| data.get_temp::<Option<bool>>(found_id())).flatten()
}

/// Whether an app asked of the charger lately, so the platform should look.
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn wanted(ctx: &egui::Context, now: f64) -> bool {
    ctx.data(|data| data.get_temp::<f64>(wanted_id())).is_some_and(|asked| now - asked <= WANTED_FOR)
}

/// What the platform found, for charging to say.
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn found(ctx: &egui::Context, charging: Option<bool>) {
    ctx.data_mut(|data| data.insert_temp(found_id(), charging));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_charger_is_unknown_until_the_platform_looked_and_asking_keeps_it_looking() {
        let ctx = egui::Context::default();

        assert!(!wanted(&ctx, 0.0), "nobody asked");
        assert_eq!(charging(&ctx), None);
        assert!(wanted(&ctx, 0.0) && !wanted(&ctx, 60.0), "asked a while, not for ever");

        found(&ctx, Some(false));
        assert_eq!(charging(&ctx), Some(false));
        found(&ctx, Some(true));
        assert_eq!(charging(&ctx), Some(true));
        found(&ctx, None);
        assert_eq!(charging(&ctx), None);
    }
}
