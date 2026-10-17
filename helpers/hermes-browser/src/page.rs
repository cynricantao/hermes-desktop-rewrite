//! The JavaScript the helper injects into the page — one small script per
//! command, each returning a plain object that becomes that command's answer.
//!
//! Deliberately small. This is NOT Electron's preview act engine
//! (`apps/desktop/src/lib/preview-act/`): no element refs, no inventory/delta
//! bookkeeping, no watch overlay, no real pointer/keystroke input — see
//! docs/browser.md for what that costs. Scripts that need a piece of the answer
//! filled in later carry a `@MARKER@` substituted in Rust.
//!
//! `hermesLabel` mirrors the engine's `labelOf` (preview-act/naming.ts) —
//! aria-label, then the aria-labelledby text, then textContent, then
//! placeholder/title/alt/name/value, clamped to 80 characters — so the `acted`
//! wording the controller builds reads like the Electron pane's.

/// Hard cap on the text one `read` brings back, in UTF-16 code units (what
/// JavaScript counts). Longer inner texts are cut and the answer says so.
pub const TEXT_CAP_UNITS: usize = 200_000;

/// Helpers every script shares: labelling and synthetic input.
///
/// The webview has no channel for real input, so clicks and keys are dispatched
/// as full bubbling event sequences. Electron's engine calls the same approach
/// its worse-but-working fallback for panes with no input channel
/// (preview-act.ts) — a page can tell them from real input (`isTrusted`), and
/// nothing moves the browser's own hover/focus target.
const HELPERS: &str = r#"
  function hermesTag(el) { return el.tagName ? el.tagName.toLowerCase() : '?'; }
  function hermesLabel(el) {
    try {
      var aria = el.getAttribute && el.getAttribute('aria-label');
      if (aria) { return String(aria).slice(0, 80); }
      var byId = el.getAttribute && el.getAttribute('aria-labelledby');
      var named = byId ? document.getElementById(byId) : null;
      var text = ((named || el).textContent || '').trim().replace(/\s+/g, ' ');
      if (text) { return text.slice(0, 80); }
      var attrs = ['placeholder', 'title', 'alt', 'name', 'value'];
      for (var i = 0; i < attrs.length; i++) {
        var value = el.getAttribute && el.getAttribute(attrs[i]);
        if (value) { return String(value).slice(0, 80); }
      }
    } catch (err) {}
    return '';
  }
  function hermesReport(el) { return { tag: hermesTag(el), label: hermesLabel(el) }; }
  function hermesMouse(el, type, x, y, buttons) {
    var init = { bubbles: true, cancelable: true, view: window, clientX: x || 0, clientY: y || 0, button: 0, buttons: buttons || 0 };
    var pointer = { mousemove: 'pointermove', mousedown: 'pointerdown', mouseup: 'pointerup' }[type];
    if (pointer) { try { el.dispatchEvent(new PointerEvent(pointer, init)); } catch (err) {} }
    el.dispatchEvent(new MouseEvent(type, init));
  }
  function hermesClick(el, x, y) {
    hermesMouse(el, 'mousemove', x, y, 0);
    hermesMouse(el, 'mousedown', x, y, 1);
    hermesMouse(el, 'mouseup', x, y, 0);
    hermesMouse(el, 'click', x, y, 0);
  }
  function hermesKey(el, key) {
    var codes = { Enter: 13, Escape: 27, Tab: 9, ' ': 32, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39 };
    var init = { key: key, code: key, keyCode: codes[key] || 0, which: codes[key] || 0, bubbles: true, cancelable: true };
    ['keydown', 'keypress', 'keyup'].forEach(function (type) { el.dispatchEvent(new KeyboardEvent(type, init)); });
  }
"#;

/// `read`: the live URL, title and `document.body.innerText` (capped at
/// [`TEXT_CAP_UNITS`]; `total` is the page's own length, so a truncated answer
/// can say by how much).
const READ: &str = r#"(function () {
  try {
    var body = document.body;
    var full = (body && body.innerText) ? body.innerText : '';
    var out = full;
    var truncated = false;
    if (full.length > @CAP@) {
      out = full.slice(0, @CAP@);
      // Never end mid-surrogate-pair: an unpaired half does not survive JSON.
      if (/[\uD800-\uDBFF]$/.test(out)) { out = out.slice(0, -1); }
      truncated = true;
    }
    return { url: location.href, title: document.title, text: out, total: full.length, truncated: truncated };
  } catch (err) { return { error: String(err) }; }
})()"#;

/// `click`: aim at the element the spec names, bring it on screen, then dispatch
/// the pointer sequence at its centre.
const CLICK: &str = r#"(function () {
@HELPERS@
  try {
    var spec = @SPEC@;
    var el = null;
    var x = 0;
    var y = 0;
    if (spec.selector != null) {
      el = document.querySelector(spec.selector);
      if (!el) { return { error: 'no element matches ' + spec.selector }; }
      el.scrollIntoView({ block: 'center', inline: 'center' });
      var box = el.getBoundingClientRect();
      x = box.left + box.width / 2;
      y = box.top + box.height / 2;
    } else {
      x = spec.x;
      y = spec.y;
      el = document.elementFromPoint(x, y);
      if (!el) { return { error: 'nothing is at (' + x + ', ' + y + ') in the viewport' }; }
    }
    hermesClick(el, x, y);
    return hermesReport(el);
  } catch (err) { return { error: String(err) }; }
})()"#;

/// `type`: commit `text` through the field's own native value setter and
/// announce it with input/change — Electron's fallback for fields that fight
/// real keystrokes (preview-act.ts `buildDirectSetScript`), which here is the
/// only path there is. `submit` presses Enter and submits the owning form.
const TYPE: &str = r#"(function () {
@HELPERS@
  try {
    var selector = @SEL@;
    var text = @TEXT@;
    var el = document.querySelector(selector);
    if (!el) { return { error: 'no element matches ' + selector }; }
    el.scrollIntoView({ block: 'center' });
    el.focus();
    var setter = null;
    if (el.tagName === 'TEXTAREA' && window.HTMLTextAreaElement) {
      setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    } else if (el.tagName === 'INPUT' && window.HTMLInputElement) {
      setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    }
    if (setter) { setter.call(el, text); }
    else if (el.isContentEditable) { el.textContent = text; }
    else if ('value' in el) { el.value = text; }
    else { return { error: hermesTag(el) + ' is not a text field' }; }
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    var submitted = false;
    if (@SUBMIT@) {
      hermesKey(el, 'Enter');
      var form = el.form || (el.closest ? el.closest('form') : null);
      if (form) {
        // Enter's implicit submission (HTML spec): click the form's default submit
        // button, so pages listening for the button's click see it too; with no
        // such button, submit the form itself.
        var dflt = form.querySelector('button:not([type]), button[type=submit], input[type=submit]');
        try {
          if (dflt) { dflt.click(); }
          else if (form.requestSubmit) { form.requestSubmit(); }
          else { form.submit(); }
          submitted = true;
        } catch (err) {}
      }
    }
    var report = hermesReport(el);
    report.submitted = submitted;
    return report;
  } catch (err) { return { error: String(err) }; }
})()"#;

/// `scroll`: window.scrollBy/To. NOT a wheel gesture — the webview cannot
/// deliver real wheel input, so page code that only listens for `wheel`
/// (scroll-linked headers, infinite-scroll loaders) stays asleep, exactly as in
/// Electron's scripted fallback.
const SCROLL: &str = r#"(function () {
  try {
    var sc = document.scrollingElement || document.documentElement;
    var range = sc ? (sc.scrollHeight - sc.clientHeight) : 0;
    if (!range || range <= 0) { return { range: 0 }; }
    var still = !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
    var behavior = still ? 'auto' : 'smooth';
    var to = @TO@;
    var amount = @AMOUNT@;
    if (to === 'top') { window.scrollTo({ top: 0, behavior: behavior }); }
    else if (to === 'bottom') { window.scrollTo({ top: sc.scrollHeight, behavior: behavior }); }
    else { window.scrollBy({ top: amount, behavior: behavior }); }
    return { range: range, to: to, amount: amount };
  } catch (err) { return { error: String(err) }; }
})()"#;

/// `wait-for-load`: where the document is right now.
const READY_STATE: &str = r#"(function () {
  try {
    return { ready_state: document.readyState, url: location.href, title: document.title };
  } catch (err) { return { error: String(err) }; }
})()"#;

/// What the mutating commands re-read once the page has settled, so their
/// answers carry the live url/title (Electron's post-action inventory does).
const IDENTITY: &str = r#"(function () {
  try {
    return { url: location.href, title: document.title };
  } catch (err) { return { error: String(err) }; }
})()"#;

/// Script for `Command::Read`.
pub fn read() -> String {
    READ.replace("@CAP@", &TEXT_CAP_UNITS.to_string())
        .replace("@HELPERS@", HELPERS)
}

/// Script for `Command::Click`: a CSS `selector`, or viewport (x, y).
pub fn click(selector: Option<&str>, x: Option<f64>, y: Option<f64>) -> String {
    let spec = match selector {
        Some(selector) => serde_json::json!({ "selector": selector }),
        None => serde_json::json!({ "x": x, "y": y }),
    };
    CLICK.replace("@HELPERS@", HELPERS).replace("@SPEC@", &spec.to_string())
}

/// Script for `Command::Type`.
pub fn type_text(selector: &str, text: &str, submit: bool) -> String {
    TYPE.replace("@HELPERS@", HELPERS)
        .replace("@SEL@", &js_string(selector))
        .replace("@TEXT@", &js_string(text))
        .replace("@SUBMIT@", if submit { "true" } else { "false" })
}

/// Script for `Command::Scroll`. A missing `amount` becomes one screen, the way
/// a wheel notch's default does in the Electron pane.
pub fn scroll(amount: Option<f64>, to: Option<&str>) -> String {
    let amount = match amount {
        Some(amount) => js_number(amount),
        None => "Math.round(window.innerHeight * 0.9)".to_owned(),
    };
    SCROLL.replace("@TO@", &js_opt_string(to)).replace("@AMOUNT@", &amount)
}

/// Script for one `wait-for-load` poll.
pub fn ready_state() -> String {
    READY_STATE.to_owned()
}

/// Script re-read after a mutating command settles.
pub fn identity() -> String {
    IDENTITY.to_owned()
}

/// A JavaScript string literal for `text` (JSON escaping is valid JS).
fn js_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

/// A JavaScript string literal, or `null` when there is nothing to encode.
fn js_opt_string(text: Option<&str>) -> String {
    match text {
        Some(text) => js_string(text),
        None => "null".to_owned(),
    }
}

/// A JavaScript number literal; non-finite values become 0 so the page hears a
/// number and not `NaN`-adjacent text.
fn js_number(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "0".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_caps_the_text_and_shields_surrogate_pairs() {
        let script = read();
        assert!(script.contains(&format!("full.length > {TEXT_CAP_UNITS}")));
        assert!(script.contains(r"\uD800-\uDBFF"), "keeps the pair guard");
        assert!(!script.contains("@CAP@"), "every marker is substituted");
    }

    #[test]
    fn click_embeds_a_selector_or_a_point() {
        let by_selector = click(Some("#go\"weird"), None, None);
        assert!(
            by_selector.contains(r##"var spec = {"selector":"#go\"weird"}"##),
            "{by_selector}"
        );
        let by_point = click(None, Some(10.0), Some(20.5));
        assert!(by_point.contains(r#"var spec = {"x":10.0,"y":20.5}"#), "{by_point}");
    }

    #[test]
    fn type_embeds_text_as_a_json_string() {
        let script = type_text("#q", "say \"hi\"\n", false);
        assert!(script.contains(r##"var selector = "#q";"##), "{script}");
        assert!(script.contains(r##"var text = "say \"hi\"\n";"##), "{script}");
        assert!(script.contains("if (false)"));
        assert!(type_text("#q", "x", true).contains("if (true)"));
    }

    #[test]
    fn scroll_defaults_to_one_screen_and_encodes_a_target() {
        assert!(scroll(None, None).contains("var amount = Math.round(window.innerHeight * 0.9);"));
        assert!(scroll(Some(-240.5), Some("top")).contains("var amount = -240.5;"));
        assert!(scroll(None, Some("bottom")).contains(r#"var to = "bottom";"#));
    }
}
