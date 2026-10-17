# INDEX.md - original-app screenshot map (light-theme captures at 1440x900 via headless sandboxed mock backend)

## orig-home
- Titled `#/`; first-launch state in this capture: white splash card (~360,360,720x180) 'Let's get you setup with Hermes Agent' + 'Starting Hermes…' + blue 100% progress bar. JSON controls show the normal chrome underneath (sidebar tabs SESSIONS(selected)/BOTS; nav rows; NEW SESSION composer row selected; status bar Backend ready/Smart). Serif 'HERMES AGENT' greeting on empty session; sans elsewhere; blue accents.
- mapped rows: ONB-001, ONB-002, CHAT-038, CHAT-030, CHAT-031, SESS-001, SESS-004, PROJ-002, PROJ-025, WIN-002, WIN-004

## orig-command-center
- `#/command-center`; modal (~48,48..1392,852) over dimmed app: modal header breadcrumb 'Command Center > Sessions' + search field [1139,71,193,28], left modal-nav Sessions(System icon: pulse; Usage: bar-chart)/Maintenance(wrench) [59,59..247,177], main pane empty 'No sessions yet.'. Sans; grey/blue accents.
- mapped rows: EXTR-019, EXTR-020

## orig-capabilities
- `#/capabilities`; top toolbar icons (~1176..1296,5); content tabs Skills 53/Tools 23/Connectors/Plugins (~260-1440,35-80); installed-skills list (~265-755) with ON toggles; skill detail panel (~780-1415,85-430); embedded 'Skills Hub' browser frame (~260-1440,440-855). Sans; blue selection, orange hub accents.
- mapped rows: SKIL-001, SKIL-002, SKIL-003, SKIL-004, TSET-001, TSET-009

## orig-messaging
- `#/messaging`; toolbar (~0-1440,0-35); platform list column (~260-470, search 'Try "matrix"' [294,42], rows y>=88); Telegram detail/form pane (~470-1440): QUICK SETUP card, GET YOUR CREDENTIALS, REQUIRED/RECOMMENDED sections, webhook secret; status bar y~880. Sans; grey uppercase labels; blue primary buttons.
- mapped rows: MSG-001, MSG-002, MSG-004, MSG-008

## orig-webhooks
- `#/webhooks`; sidebar as usual; main pane: 'Webhook receiver disabled' notice + explainer, 'Enable webhooks' [114,150,131,24], empty 'No webhook subscriptions yet.', disabled 'New subscription' [656,538,127,24]. Sans; grey/blue.
- mapped rows: EXTR-001, EXTR-002

## orig-artifacts
- `#/artifacts`; filter tabs 'All 0/Images 0/Files 0/Links 0' (~751-949,42) with count badges, refresh icon [1404,44]; centered empty 'No artifacts found' + helper (~640-1060,460-500). Sans; blue underline accents.
- mapped rows: ART-001

## orig-cron
- `#/cron` modal over dimmed app: header 'Scheduled jobs'/'0 jobs', search [91,107], '+' empty-state hint (~157), BLUEPRINTS list of 16 rows (~69,242..270, each 28px pitch), central empty state with stopwatch icon + explainer + blue 'New cron' [799,515]. Sans; grey/blue.
- mapped rows: CRON-001, CRON-002, CRON-008, CRON-010

## orig-profiles
- `#/profiles`-like modal 'Profiles': header + '1 profile' count + x close; profile list w/ search [68,108] and selected 'default' [68,140], '+' new [~173,182]; detail pane Model/Skills rows, SOUL.MD editor (monospace) + blue Save [1243,455]; 'Default' badge blue / 'Env' grey outline. Sans body + mono code.
- mapped rows: PROF-001, PROF-002, PROF-005, PROF-006, PROF-020

## orig-agents
- `#/agents` modal over dimmed app: only empty state - bot icon, 'No live subagents', caption (~535-905,408-483); toolbar x close top-right; sans, off-white modal.
- mapped rows: EXTR-006

## orig-starmap
- `#/starmap` modal over dimmed app: empty state - lightbulb icon, 'Nothing learned yet', caption 'As Hermes builds skills and memories...'; no nodes/legend; only toolbar x close.
- mapped rows: EXTR-017

## orig-session-import
- `#/session-import` modal 'Continue from another app': header title+subtitle, meta row 'Reading from cynric-omarchy' (monitor icon) + 'Import into default', refresh + x; left list pane (light grey) with segmented All(selected)/Claude Code/Codex and empty 'No conversations found'; right preview pane empty 'A conversation worth continuing'; serif-none, ~28px bold title.
- mapped rows: EXTR-005, SSAF-008

## orig-settings-tab-config-model-page-main
- Settings modal (48,48..1392,852) over dimmed app + floating 'Search Ctrl K' pill (~655,36); left sidebar nav (~48-256): expandable groups w/ chevrons, active page light-blue highlight; content pane (~256-1392) breadcrumb + forms to x~1332; footer icon row ~y815-838. Page: breadcrumb 'Settings > Model > Main model'; selects 'custom'/'mock-model', Apply, Reasoning Medium, context override 64000; Model group expanded w/ 4 sub-items; nav Chat..About below.
- mapped rows: SMOD-001, SMOD-002, SMOD-003, MODL-001

## orig-settings-tab-config-model-page-fallbacks
- Settings modal (48,48..1392,852) over dimmed app + floating 'Search Ctrl K' pill (~655,36); left sidebar nav (~48-256): expandable groups w/ chevrons, active page light-blue highlight; content pane (~256-1392) breadcrumb + forms to x~1332; footer icon row ~y815-838. Page: 'Fallback Models' heading + explainer + empty note + '+ Add fallback'.
- mapped rows: SMOD-005

## orig-settings-tab-config-model-page-auxiliary
- Settings modal (48,48..1392,852) over dimmed app + floating 'Search Ctrl K' pill (~655,36); left sidebar nav (~48-256): expandable groups w/ chevrons, active page light-blue highlight; content pane (~256-1392) breadcrumb + forms to x~1332; footer icon row ~y815-838. Page: 'Reset all to main'; 10 auxiliary task rows all 'auto · use main model'.
- mapped rows: SMOD-006

## orig-settings-tab-config-model-page-moa
- Settings modal (48,48..1392,852) over dimmed app + floating 'Search Ctrl K' pill (~655,36); left sidebar nav (~48-256): expandable groups w/ chevrons, active page light-blue highlight; content pane (~256-1392) breadcrumb + forms to x~1332; footer icon row ~y815-838. Page: MoA config - preset bar, refs 1/2 w/ toggles, Add reference model, aggregator row.
- mapped rows: SMOD-007

## orig-settings-tab-config-chat-page-behavior
- Settings modal (48,48..1392,852) over dimmed app + floating 'Search Ctrl K' pill (~655,36); left sidebar nav (~48-256): expandable groups w/ chevrons, active page light-blue highlight; content pane (~256-1392) breadcrumb + forms to x~1332; footer icon row ~y815-838. Page: Personality/None, Timezone combobox, Reasoning Blocks toggle ON; Chat expanded w/ Behavior+Attachments.
- mapped rows: SCHT-001, SCHT-002, SCHT-003

## orig-settings-tab-config-chat-page-attachments
- Settings modal (48,48..1392,852) over dimmed app + floating 'Search Ctrl K' pill (~655,36); left sidebar nav (~48-256): expandable groups w/ chevrons, active page light-blue highlight; content pane (~256-1392) breadcrumb + forms to x~1332; footer icon row ~y815-838. Page: 'Max preview / image load size' 16 MB input + 'Image Attachments' select 'Auto'.
- mapped rows: SCHT-004, SCHT-005

## orig-settings-tab-config-appearance-page-general
- Appearance shell: sidebar w/ Appearance expanded (6 children), General active pill; content rows Language..Guided Tours toggles at x~1297-1333.
- mapped rows: SAPP-001, SAPP-002, SAPP-003, SAPP-004, SAPP-005, SAPP-006

## orig-settings-tab-config-appearance-page-theme
- Theme page: Light/Dark/System segmented (System), VS Code Marketplace search, 12-card theme grid (Nous selected). Appearance active in nav.
- mapped rows: SAPP-007, SAPP-008, THEM-001, THEM-002

## orig-settings-tab-config-appearance-page-typography
- Typography page: UI Scale seg 100%, Chat Text Size seg 110%, Chat Font combo + preview, Terminal Font combo + glyph preview.
- mapped rows: SAPP-009, SAPP-010, SAPP-011, SAPP-012

## orig-settings-tab-config-appearance-page-window-layout
- Window & layout page: Interface mode (Advanced), Session List Density (Compact), Tab Strip (Auto), App Actions (Right), 4 toggles off (tray, Chat Backdrop statue, File Browser, Floating Composer).
- mapped rows: SAPP-013, SAPP-014, SAPP-015, SAPP-016, SAPP-017, SAPP-019, SAPP-020, SAPP-021

## orig-settings-tab-config-appearance-page-chat-display
- Chat display page in Appearance shell: rows slider 0%, direction Auto, toggles OFF/OFF/ON(P.OFF/OFF/OFF/OFF), Tool display Product, Embeds Ask.
- mapped rows: SAPP-022, SAPP-023, SAPP-024, SAPP-025, SAPP-026, SAPP-027, SAPP-028, SAPP-029, SAPP-030

## orig-settings-tab-config-appearance-page-pet
- Pet page: failed gallery empty state ('Couldn't reach petdex…', '0 pets.'), Choose a pet toggle OFF, search input.
- mapped rows: SAPP-031, WIN-018

## orig-settings-tab-config-workspace-page-projects
- Workspace expanded (Projects & discovery active): Working Directory '.', repo-discovery toggle ON, roots/exclude placeholder inputs.
- mapped rows: SWRK-001, SWRK-002, SWRK-003, SWRK-004

## orig-settings-tab-config-workspace-page-shell
- Workspace > Shell environment active: Persistent Shell toggle ON, Environment Passthrough empty input; only 2 rows.
- mapped rows: SWRK-006, SWRK-007

## orig-settings-tab-config-workspace-page-files
- Workspace > Files & execution active: Code Execution Mode 'Project', File Read Limit 100000; only 2 rows.
- mapped rows: SWRK-005, SWRK-008

## orig-settings-tab-config-safety-page-approvals
- Safety expanded (Approvals active): Approval Mode 'Off', Timeout 300, MCP reload toggle ON, allowlist empty.
- mapped rows: SSAF-001, SSAF-002, SSAF-003, SSAF-004

## orig-settings-tab-config-safety-page-privacy
- Safety > Privacy & network active: 4 toggle rows (collect/share usage OFF+disabled, Redact Secrets ON, Private URLs OFF); 'What is counted' link.
- mapped rows: SSAF-005, SSAF-006, SSAF-008, EXTR-021

## orig-settings-tab-config-safety-page-checkpoints
- Safety > Checkpoints active: single File Checkpoints row, toggle OFF; rest empty.
- mapped rows: SSAF-007

## orig-settings-tab-config-browser-page-profile
- Browser expanded (Browser profile active): single 'Use My Real Browser Profile' row, toggle OFF.
- mapped rows: SBRW-001

## orig-settings-tab-config-browser-page-network
- Browser > Local & private URLs active: 2 toggles (Private URLs OFF, Local browser ON).
- mapped rows: SBRW-002, SBRW-003

## orig-settings-tab-config-memory-page-persistent
- Memory & Context expanded (Persistent memory active): 5 rows - toggles ON/ON, budgets 2200/1375, provider 'Built-in only'.
- mapped rows: SMEM-001, SMEM-002, SMEM-003, SMEM-004, SMEM-005

## orig-settings-tab-config-memory-page-context
- Memory & Context > Context & compression active: 7 rows (engine Compressor, toggles ON, 0.5/0.2/20/120 inputs).
- mapped rows: SMEM-006, SMEM-007, SMEM-008, SMEM-009, SMEM-010, SMEM-011, SMEM-012

## orig-settings-tab-config-voice-page-conversation
- Voice expanded (Voice conversation active, mic icon): rows Voice Chat Mode 'Chained', GPT-Live Voice 'marin', Persona 'Not set', Max Recording 120, Client Direct ON.
- mapped rows: SVOI-001, SVOI-002, SVOI-003, SVOI-017, SVOI-018

## orig-settings-tab-config-voice-page-transcription
- Voice > Speech to text active: STT ON, Echo ON, Live OFF, model Base, language Not set; shared shortcut block.
- mapped rows: SVOI-005, SVOI-006, SVOI-008, SVOI-015

## orig-settings-tab-config-voice-page-speech
- Voice > Text to speech active: TTS provider 'Edge', Read aloud OFF, Edge Voice 'en-US-AriaNeural'.
- mapped rows: SVOI-004, SVOI-009, SVOI-010

## orig-settings-tab-config-advanced-page-desktop
- Advanced expanded (Desktop & startup active): Keep awake Off, F12 OFF, links OFF, Quick Entry ON w/ shortcut 'CommandOrControl+Shift+S', Run again button, Update Local Changes 'Stash'.
- mapped rows: SADV-013, SADV-014, SADV-015, SADV-018, SADV-019, SADV-020, SADV-012

## orig-settings-tab-config-advanced-page-runtime
- Advanced > Agent limits active: 3 rows (steps Not set, retries 3, Service Tier '(none)').
- mapped rows: SADV-007, SADV-008, SADV-009

## orig-settings-tab-config-advanced-page-tools
- Advanced > Tool access active: 2 rows (Enabled Toolsets 'hermes-cli', Tool-Use Enforcement 'auto').
- mapped rows: SADV-001, SADV-010

## orig-settings-tab-config-advanced-page-terminal
- Advanced > Terminal backend active: backend 'Local', timeout 180, 4 container-image rows truncated.
- mapped rows: SADV-002, SADV-003, SADV-004

## orig-settings-tab-config-advanced-page-delegation
- Advanced > Subagents active: 6 delegation rows (Not set x2, 250, 10, 0, '(none)').
- mapped rows: SADV-011

## orig-settings-tab-config-advanced-page-output
- Advanced > Output limits active: 4 numeric inputs (50000/2000/2000/20).
- mapped rows: SADV-005, SADV-006

## orig-settings-tab-providers
- Providers expanded (Accounts active): Connect an account, Nous Portal RECOMMENDED card, Run models locally, Other providers ⌄; sub-items Accounts/API keys/Custom Endpoints/Local Models.
- mapped rows: SPRV-001, SPRV-002, SPRV-003, SPRV-006, MODL-023

## orig-settings-tab-gateway-page-connection
- Gateways expanded ('This window' active): 4 connection-mode cards (Local selected), keychain toggle OFF, Diagnostics/Open logs.
- mapped rows: SGTW-001, SGTW-006, SGTW-012

## orig-settings-tab-gateway-page-devices
- Gateways > Saved connections active: This device row w/ Current/Primary/App-managed pills + Test, Add connection, startup-restore toggle OFF.
- mapped rows: RMT-013, RMT-015, SGTW-007

## orig-settings-tab-gateway-page-managed-updates
- Gateways > Remote updates active: empty state 'Managed updates' + SSH helper; child rows This window/Saved connections/Remote updates.
- mapped rows: UPD-006

## orig-settings-tab-keybinds-page-shortcuts
- Keyboard Shortcuts > Key bindings active: capture hint + Reset all + search + grouped keycap rows (COMPOSER/PROFILES), 'set' chips for unbound.
- mapped rows: SKEY-001

## orig-settings-tab-keybinds-page-hud-gesture
- Keyboard Shortcuts expanded (HUD gesture active): single 'Tap to summon HUD' row, toggle OFF.
- mapped rows: SKEY-002

## orig-settings-tab-keybinds-page-screen-capture
- Keyboard Shortcuts > Screen capture active: breadcrumb only, empty content.
- mapped rows: SKEY-003

## orig-settings-tab-keys
- Tools & Keys expanded (Tools active): flat uppercase tool-key row list with placeholders.
- mapped rows: SPRV-005

## orig-settings-tab-vault-page-credentials
- Passwords & Logins expanded (Saved credentials active): + Add button, explainer, 'Nothing saved yet' empty state.
- mapped rows: SVLT-001, SVLT-002

## orig-settings-tab-vault-page-sources
- Passwords & Logins > Password managers active: 1Password/Bitwarden 'Not detected' rows.
- mapped rows: SPRV-005, SVLT-005

## orig-settings-tab-notifications-page-alerts
- Notifications expanded (Desktop alerts active): 8 toggles all ON, 'Send test notification' button.
- mapped rows: SNOT-001, SNOT-002, SNOT-003, SNOT-004, SNOT-005, SNOT-006, SNOT-007, SNOT-008, NOTI-011

## orig-settings-tab-notifications-page-sounds
- Notifications > Sounds active: Completion Sound select 'Two-note comfort' + Preview button.
- mapped rows: SNOT-009, NOTI-010

## orig-settings-tab-billing
- Billing expanded (Overview active): signed-out banner + Balance/Plan/Auto-refill dashes.
- mapped rows: SBIL-001, SBIL-003

## orig-settings-tab-sessions-page-archived
- Sessions expanded (Archive & retention active): Auto-archive OFF, archived list empty state 'Nothing archived'.
- mapped rows: SSES-001, SSES-003

## orig-settings-tab-sessions-page-default-directory
- Sessions > Default project folder active: 'Not set' + Choose button.
- mapped rows: SSES-002

## orig-settings-tab-plugins
- Plugins active (single page, no children): empty state + 'Manage plugins →'.
- mapped rows: SPLG-001, SPLG-003

## orig-settings-tab-about-page-updates
- About expanded (Version & updates active): identity block v0.22.1+683, red failed-update card w/ Check now, details table.
- mapped rows: SABT-001, SABT-003, UPD-005

## orig-settings-tab-about-page-uninstall
- About > Uninstall active: pink danger card, one 'Uninstall Chat GUI only' row w/ trash icon.
- mapped rows: SABT-004, SABT-005

## orig-chat-reply
- `#/local-01...dd`; tab row with active 'HELLO FROM THE PARITY CAP…' tab + '+' [464,17], right icon cluster (settings/book/share/swap/panel ~1188-1340); sidebar: SESSIONS/BOTS tabs, nav rows w/ icons, search [40,224], PINNED + SESSIONS headers, selected session row 'now' chip; transcript (260,34..~800): user bordered card (285,44), assistant plain text y~97, right-edge scroll handle (1424,457); consent card strip (~280,800..1420,826); composer (280,830..1420,872) '+' + placeholder 'Give Hermes a task' + 'Mock Model⌄'/'Med⌄' + mic + dark round send; status bar: 'Backend ready', gateway id, 'Smart', 'v0.22.1+683'.
- mapped rows: TRNS-001, TRNS-002, SESS-001, SESS-013, SESS-012, SESS-016, SESS-021, SESS-005, WIN-004, SSAF-008

## orig-chat-tool-row
- Same layouts as orig-chat-reply; transcript additionally: tool call card [284,151] with mono raw E2E_CALL invocation, grey '>'-glyph result row 'Ran echo parity-tool-row' [296,201], plain-text E2E_CALL_RESULT body line; similar scroll/two-handle marks at y~264.
- mapped rows: TOOL-001, TOOL-002, TOOL-004, TOOL-021, TRNS-002, SESS-001, SESS-013, SESS-012, SESS-016, SESS-021, SESS-005, WIN-004, SSAF-008
