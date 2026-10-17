// Parity capture: drives the ORIGINAL Hermes Desktop (Electron, built dist/) inside a private
// headless Weston, against the e2e sandbox (own HOME/HERMES_HOME, isolated backend, mock
// inference server). For every screen it writes <name>.png plus <name>.json: a compact
// inventory of visible text and interactive elements (role, label, title, state).
// Nothing here touches the user's real backend, config, sessions or display.
import fs from 'node:fs'
import path from 'node:path'

import { spawn } from 'node:child_process'

import { chromium } from '@playwright/test'

const DESKTOP = path.join(process.env.HERMES_AGENT_ROOT || path.join(process.env.HOME, '.hermes/hermes-agent'), 'apps/desktop')
const OUT = process.env.PARITY_OUT || path.join(import.meta.dirname, 'out')
const ONLY = process.env.PARITY_ONLY || ''

import os from 'node:os'

// Same isolation as apps/desktop/e2e/fixtures.ts (which can't be imported outside the
// Playwright runner): own HOME + HERMES_HOME + userData, isolated backend, no inherited
// credentials or HERMES_* runtime vars.
const ELECTRON = path.join(DESKTOP, 'node_modules/electron/dist/electron')
const findElectron = () => ELECTRON
const CRED = ['_API_KEY', '_TOKEN', '_SECRET', '_PASSWORD', '_CREDENTIALS', '_ACCESS_KEY', '_PRIVATE_KEY', '_OAUTH_TOKEN']
const CRED_NAMES = new Set(['ANTHROPIC_BASE_URL', 'ANTHROPIC_TOKEN', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_SESSION_TOKEN', 'CUSTOM_API_KEY', 'GEMINI_BASE_URL', 'OPENAI_BASE_URL', 'OPENROUTER_BASE_URL', 'OLLAMA_BASE_URL', 'GROQ_BASE_URL', 'XAI_BASE_URL'])
function createSandbox(prefix) {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), `hermes-parity-${prefix}-`)))
  const hermesHome = path.join(root, 'hermes-home'); const userDataDir = path.join(root, 'electron-user-data')
  fs.mkdirSync(hermesHome, { recursive: true }); fs.mkdirSync(userDataDir, { recursive: true })
  return { root, hermesHome, userDataDir, cleanup: () => fs.rmSync(root, { recursive: true, force: true }) }
}
function buildAppEnv(sb) {
  const clean = {}
  for (const [k, v] of Object.entries(process.env)) {
    if (!v || CRED_NAMES.has(k) || CRED.some(x => k.endsWith(x))) continue
    if (k.startsWith('HERMES_') && !k.startsWith('HERMES_DESKTOP_') && !k.startsWith('HERMES_E2E_')) continue
    clean[k] = v
  }
  return { ...clean, HERMES_HOME: sb.hermesHome, HERMES_DESKTOP_USER_DATA_DIR: sb.userDataDir, HERMES_DESKTOP_IGNORE_EXISTING: '1',
    HERMES_DESKTOP_ISOLATED_BACKEND: '1', HOME: sb.root, HERMES_DESKTOP_HERMES_ROOT: path.resolve(DESKTOP, '../..'),
    HERMES_DESKTOP_APP_NAME: `HermesParity-${Date.now()}`, HERMES_DESKTOP_SKIP_QUIT_CONFIRM: '1', HERMES_DESKTOP_SKIP_PROTOCOL_REGISTRATION: '1' }
}
const { writeEnvFile, writeMockProviderConfig } = await import(path.join(DESKTOP, '../../tests-js/scripts/mock-provider-config.ts'))
const { startMockServer } = await import(path.join(DESKTOP, '../../tests-js/scripts/mock-server.ts'))
const CDP_PORT = Number(process.env.PARITY_CDP_PORT || 9339)

// Playwright's _electron.launch dies against this app on a seatless headless Weston, so the
// app is started as a plain process with a loopback CDP port and attached over CDP instead.
async function setupMockBackend() {
  try { await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`); throw new Error(`port ${CDP_PORT} already in use; refusing to attach to a stray instance`) } catch (e) { if (String(e).includes('refusing')) throw e }
  const mock = await startMockServer()
  const sandbox = createSandbox('parity')
  writeMockProviderConfig(sandbox.hermesHome, mock.url)
  writeEnvFile(sandbox.hermesHome)
  const env = buildAppEnv(sandbox)
  const proc = spawn(findElectron(), [DESKTOP, '--disable-gpu', '--no-sandbox', '--ozone-platform=wayland', `--remote-debugging-port=${CDP_PORT}`], { env, cwd: DESKTOP, stdio: ['ignore', 'pipe', 'pipe'], detached: true })
  proc.stdout.on('data', d => process.env.PARITY_VERBOSE && process.stdout.write(`[app] ${d}`))
  proc.stderr.on('data', d => process.env.PARITY_VERBOSE && process.stdout.write(`[app!] ${d}`))
  let browser
  for (let i = 0; i < 360 && !browser; i++) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`) } catch { await new Promise(r => setTimeout(r, 500)) }
  }
  if (!browser) throw new Error('CDP port never opened')
  let page
  for (let i = 0; i < 120 && !page; i++) {
    page = browser.contexts().flatMap(c => c.pages()).find(p => /index\.html/.test(p.url()))
    if (!page) await new Promise(r => setTimeout(r, 500))
  }
  if (!page) throw new Error('main window never appeared: ' + browser.contexts().flatMap(c => c.pages()).map(p => p.url()).join(', '))
  const cleanup = async () => { await browser.close().catch(() => {}); try { process.kill(-proc.pid, 'SIGTERM') } catch {}
    for (let i = 0; i < 20 && proc.exitCode === null && proc.signalCode === null; i++) await new Promise(r => setTimeout(r, 500))
    try { process.kill(-proc.pid, 'SIGKILL') } catch {}; await mock.close().catch(() => {}); sandbox.cleanup?.() }
  return { page, browser, proc, cleanup }
}

async function waitForAppReady(fixture, timeout) {
  const { page } = fixture
  await page.locator('[data-slot="composer-rich-input"]').first().waitFor({ timeout })
  // The composer mounts before the backend handshake; wait until the status pill stops
  // saying offline and the model picker has a label.
  await page.waitForFunction(() => !document.body.innerText.includes('Backend offline') &&
    !document.querySelector('[aria-label="Open model picker"]')?.hasAttribute('disabled'), null, { timeout })
  try {
    const s = await page.context().newCDPSession(page)
    const { windowId } = await s.send('Browser.getWindowForTarget')
    await s.send('Browser.setWindowBounds', { windowId, bounds: { left: 0, top: 0, width: 1440, height: 900 } })
  } catch (e) { console.log('resize failed', String(e).slice(0, 120)) }
  await page.waitForTimeout(1000)
}

const INVENTORY = `(() => {
  const vis = el => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0.01 }
  const clip = (s, n = 120) => (s || '').replace(/\\s+/g, ' ').trim().slice(0, n)
  const controls = []
  const seen = new Set()
  for (const el of document.querySelectorAll('button,a[href],input,textarea,select,[role=button],[role=switch],[role=checkbox],[role=tab],[role=menuitem],[role=option],[role=combobox],[role=slider],[role=radio],[contenteditable=true]')) {
    if (!vis(el)) continue
    const r = el.getBoundingClientRect()
    const row = {
      tag: el.tagName.toLowerCase(), role: el.getAttribute('role') || undefined,
      label: clip(el.getAttribute('aria-label')) || undefined, title: clip(el.getAttribute('title')) || undefined,
      text: clip(el.innerText || el.value || el.getAttribute('placeholder'), 80) || undefined,
      type: el.getAttribute('type') || undefined, slot: el.getAttribute('data-slot') || undefined,
      state: [el.disabled || el.getAttribute('aria-disabled') === 'true' ? 'disabled' : '',
              el.getAttribute('aria-checked') === 'true' || el.checked ? 'checked' : '',
              el.getAttribute('aria-pressed') === 'true' ? 'pressed' : '',
              el.getAttribute('aria-selected') === 'true' || el.getAttribute('data-state') === 'active' ? 'selected' : '',
              el.getAttribute('aria-expanded') === 'true' ? 'expanded' : ''].filter(Boolean).join(',') || undefined,
      kbd: clip(el.getAttribute('aria-keyshortcuts')) || undefined,
      box: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)]
    }
    const k = JSON.stringify([row.tag, row.label, row.title, row.text, row.box])
    if (seen.has(k)) continue
    seen.add(k); controls.push(row)
    if (controls.length >= 400) break
  }
  const texts = []
  const tw = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
  const tseen = new Set()
  while (tw.nextNode() && texts.length < 500) {
    const t = clip(tw.currentNode.textContent, 160)
    const p = tw.currentNode.parentElement
    if (!t || tseen.has(t) || !p || !vis(p)) continue
    tseen.add(t); texts.push(t)
  }
  return { hash: location.hash, title: document.title, w: innerWidth, h: innerHeight, texts, controls }
})()`

const shots: { name: string; hash: string; controls: number; texts: number }[] = []

async function capture(page, name: string) {
  await page.waitForTimeout(900)
  fs.mkdirSync(OUT, { recursive: true })
  await page.screenshot({ path: path.join(OUT, `${name}.png`) })
  const inv = await page.evaluate(INVENTORY)
  fs.writeFileSync(path.join(OUT, `${name}.json`), JSON.stringify(inv, null, 1))
  shots.push({ name, hash: inv.hash, controls: inv.controls.length, texts: inv.texts.length })
  console.log(`captured ${name} ${inv.hash} controls=${inv.controls.length} texts=${inv.texts.length}`)
}

async function go(page, hashPath: string) {
  await page.evaluate(h => { location.hash = h }, `#${hashPath}`)
  await page.waitForTimeout(700)
}

const slug = (s: string) => s.replace(/^\//, '').replace(/[?&=:]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '') || 'home'

async function main() {
  const fixture = await setupMockBackend()
  const { page } = fixture
  try {
    await waitForAppReady(fixture, 120_000)
    // Fixed viewport so original and rewrite screenshots line up (kiosk shell fills 1440x900).
    await page.waitForTimeout(800)

    const want = (n: string) => !ONLY || n.includes(ONLY)

    if (want('orig-home')) await capture(page, 'orig-home')

    for (const r of ['/command-center', '/capabilities', '/messaging', '/webhooks', '/artifacts', '/cron', '/profiles', '/agents', '/starmap', '/session-import']) {
      const name = `orig-${slug(r)}`
      if (!want(name)) continue
      await go(page, r)
      await capture(page, name)
    }

    const settings: string[] = JSON.parse(fs.readFileSync(path.join(process.env.PARITY_SRC_DIR || import.meta.dirname, 'settings-urls.json'), 'utf8'))
    for (const u of settings) {
      const name = `orig-${slug(u)}`
      if (!want(name)) continue
      await go(page, u)
      await capture(page, name)
    }

    if (want('orig-chat')) {
      await go(page, '/')
      const composer = page.locator('[data-slot="composer-rich-input"]').first()
      await composer.click()
      await page.keyboard.type('Hello from the parity capture')
      await page.keyboard.press('Enter')
      await page.waitForTimeout(4000)
      await capture(page, 'orig-chat-reply')
      await composer.click()
      await page.keyboard.type('E2E_CALL(terminal)[{"command":"echo parity-tool-row"}]')
      await page.keyboard.press('Enter')
      await page.waitForTimeout(8000)
      await capture(page, 'orig-chat-tool-row')
    }
  } finally {
    fs.writeFileSync(path.join(OUT, 'index.json'), JSON.stringify(shots, null, 1))
    await fixture.cleanup()
  }
}

await main()
process.exit(0)
