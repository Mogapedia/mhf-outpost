<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'

// ── Types ────────────────────────────────────────────────────────────────────

interface Version {
  id: string
  name: string
  description: string
  platform: string
  generation: 'Season' | 'Forward' | 'G' | 'Z' | null
  released: string | null
  features: string[]
  languages: string[]
  has_archive: boolean
  archive_size_gb: number | null
  archive_format: string | null
}

/// Short labels for language codes shown on version cards / detail panes.
/// MHF's three official regions are JP (Capcom Online Games), TW (Capcom
/// Taiwan) and KR (Hangame); `en` marks community English fan patches.
const LANG_LABELS: Record<string, string> = {
  'ja':    'JP',
  'zh-TW': 'TW',
  'zh-CN': 'CN',
  'ko':    'KR',
  'en':    'EN',
}
function langLabel(code: string): string {
  return LANG_LABELS[code] ?? code.toUpperCase()
}

/// Sidebar groups, shown in this order. Each generation's versions are
/// sorted newest-first via the order returned by list_versions().
const GENERATIONS: Array<{ key: 'Z' | 'G' | 'Forward' | 'Season' | 'Other'; label: string }> = [
  { key: 'Z',       label: 'Z series' },
  { key: 'G',       label: 'G series' },
  { key: 'Forward', label: 'Forward' },
  { key: 'Season',  label: 'Season' },
  { key: 'Other',   label: 'Other' },
]

/// Version we suggest to first-time users: the latest content patch with the
/// most active community translations. Drives the "Recommended" badge in the
/// sidebar and the "Get started" CTA on the welcome screen.
const RECOMMENDED_ID = 'ZZ'

interface CheckItem {
  name: string
  status: 'ok' | 'warning' | 'error'
  detail: string
  fix: string | null
}

interface ProgressEvent {
  version: string
  phase: string
  bytes_done: number
  bytes_total: number
  message: string | null
}

interface CharDto { id: number; name: string; hr: number; gr: number; is_female: boolean }

// ── State ────────────────────────────────────────────────────────────────────

const versions = ref<Version[]>([])
const selectedId = ref<string | null>(null)
const view = ref<'library' | 'server' | 'translations' | 'checks'>('library')

// Sidebar generation groups: Z and G open by default; older tiers collapsed.
const openGroups = ref<Record<string, boolean>>({
  Z: true, G: true, Forward: false, Season: false, Other: false,
})
function toggleGroup(key: string) {
  openGroups.value[key] = !openGroups.value[key]
}

/// Animate `<Transition>` enter by expanding height from 0 to scrollHeight.
/// We bypass CSS classes (:css="false") because the natural height is
/// content-dependent and can't be expressed in a static stylesheet.
const GROUP_ANIM_MS = 180
function onGroupEnter(el: Element, done: () => void) {
  const e = el as HTMLElement
  e.style.overflow = 'hidden'
  e.style.height = '0px'
  e.style.opacity = '0'
  // Force a reflow before measuring the final height, otherwise browsers
  // batch the 0->auto transition and we get no animation.
  const target = e.scrollHeight
  requestAnimationFrame(() => {
    e.style.transition = `height ${GROUP_ANIM_MS}ms ease, opacity ${GROUP_ANIM_MS}ms ease`
    e.style.height = target + 'px'
    e.style.opacity = '1'
  })
  const cleanup = () => {
    e.style.transition = ''
    e.style.height = ''
    e.style.overflow = ''
    e.style.opacity = ''
    e.removeEventListener('transitionend', onEnd)
    done()
  }
  const onEnd = (ev: TransitionEvent) => { if (ev.propertyName === 'height') cleanup() }
  e.addEventListener('transitionend', onEnd)
}
function onGroupLeave(el: Element, done: () => void) {
  const e = el as HTMLElement
  e.style.overflow = 'hidden'
  e.style.height = e.scrollHeight + 'px'
  e.style.opacity = '1'
  e.style.transition = `height ${GROUP_ANIM_MS}ms ease, opacity ${GROUP_ANIM_MS}ms ease`
  requestAnimationFrame(() => {
    e.style.height = '0px'
    e.style.opacity = '0'
  })
  const cleanup = () => {
    e.style.transition = ''
    e.style.height = ''
    e.style.overflow = ''
    e.style.opacity = ''
    e.removeEventListener('transitionend', onEnd)
    done()
  }
  const onEnd = (ev: TransitionEvent) => { if (ev.propertyName === 'height') cleanup() }
  e.addEventListener('transitionend', onEnd)
}

// Per-version installed paths (stored in localStorage)
const installedPaths = ref<Record<string, string>>(
  JSON.parse(localStorage.getItem('installedPaths') || '{}')
)
function savePaths() {
  localStorage.setItem('installedPaths', JSON.stringify(installedPaths.value))
}

// The version the user most recently launched. Drives the top-bar Play button
// so returning users hit "Play" from anywhere without going through Library.
// Null on first run (no button shown).
const lastPlayedId = ref<string | null>(localStorage.getItem('lastPlayedId'))
function saveLastPlayed(id: string) {
  lastPlayedId.value = id
  localStorage.setItem('lastPlayedId', id)
}

// First-run welcome screen. Shown when the user has neither dismissed it nor
// installed any version yet — once either is true, the launcher goes straight
// to the Library on subsequent opens.
const welcomeDismissed = ref(localStorage.getItem('welcomeDismissed') === '1')
function dismissWelcome() {
  welcomeDismissed.value = true
  localStorage.setItem('welcomeDismissed', '1')
}

// System checks
const checks = ref<CheckItem[]>([])
const checksLoading = ref(false)
const checkGamePath = ref('')

// Download state
const downloading = ref<Record<string, { phase: string; progress: number; message: string }>>({})

// Verify state (per version)
interface VerifyResult {
  ok: boolean
  ok_count: number
  placeholder_count: number
  failure_count: number
  modified_count: number
  failures: string[]
}
const verifying = ref<Record<string, boolean>>({})
const verifyResults = ref<Record<string, VerifyResult>>({})

// ── Server / auth state ───────────────────────────────────────────────────────
// Global — independent of which client version is selected.

const serverUrl = ref(localStorage.getItem('serverUrl') ?? 'http://127.0.0.1:8080')
function saveServerUrl() { localStorage.setItem('serverUrl', serverUrl.value) }

// Auth session kept in memory only (tokens expire; not persisted to localStorage)
const authStep = ref<'credentials' | 'characters' | 'done'>('credentials')
const authAction = ref<'login' | 'register'>('login')
const authUsername = ref('')
const authPassword = ref('')
const authLoading = ref(false)
const authError = ref('')
const authSession = ref('')           // opaque LoginResponse JSON from backend
const authChars = ref<CharDto[]>([])
const activeChar = ref<CharDto | null>(null)   // chosen character for this session

function resetAuth() {
  authStep.value = 'credentials'
  authAction.value = 'login'
  authUsername.value = ''
  authPassword.value = ''
  authError.value = ''
  authSession.value = ''
  authChars.value = []
  activeChar.value = null
}

async function submitCredentials() {
  authLoading.value = true
  authError.value = ''
  try {
    const result = await invoke<{ characters: CharDto[]; session_json: string }>('authenticate', {
      server: serverUrl.value,
      username: authUsername.value,
      password: authPassword.value,
      action: authAction.value,
    })
    authSession.value = result.session_json
    authChars.value = result.characters
    if (result.characters.length === 1) {
      selectChar(result.characters[0])
    } else if (result.characters.length === 0) {
      // No characters yet — go straight to character creation
      await createAndSelectChar()
    } else {
      authStep.value = 'characters'
    }
  } catch (e: any) {
    authError.value = e
  } finally {
    authLoading.value = false
  }
}

function createAndSelectChar() {
  // char_id = 0 is the sentinel for "create a new character at launch time"
  selectChar({ id: 0, name: 'New character', hr: 0, gr: 0, is_female: false })
}

function selectChar(char: CharDto) {
  activeChar.value = char
  authStep.value = 'done'
  showToast(`Ready — playing as ${char.name}`)
}

// ── Translation state ────────────────────────────────────────────────────────

const transLang = ref(localStorage.getItem('transLang') ?? 'fr')
const transRepo = ref(localStorage.getItem('transRepo') ?? 'mogapedia/MHFrontier-Translation')
const transLoading = ref(false)
const transResult = ref<{ json_path: string; release_tag: string } | null>(null)
const transError = ref('')

function saveTransPrefs() {
  localStorage.setItem('transLang', transLang.value)
  localStorage.setItem('transRepo', transRepo.value)
}

async function downloadTranslations() {
  if (!selectedPath.value) {
    showToast('Set an install path in the Library tab first', 'err')
    return
  }
  transLoading.value = true
  transError.value = ''
  transResult.value = null
  saveTransPrefs()
  try {
    transResult.value = await invoke<{ json_path: string; release_tag: string }>('download_translations', {
      gameDir: selectedPath.value,
      lang: transLang.value,
      repo: transRepo.value,
    })
    showToast('Translation data downloaded')
  } catch (e: any) {
    transError.value = String(e)
    showToast('Translation download failed', 'err')
  } finally {
    transLoading.value = false
  }
}

// Toast / status message
const toast = ref<{ text: string; type: 'ok' | 'err' } | null>(null)
function showToast(text: string, type: 'ok' | 'err' = 'ok') {
  toast.value = { text, type }
  setTimeout(() => { toast.value = null }, 4000)
}

// ── Computed ──────────────────────────────────────────────────────────────────

const selected = computed(() => versions.value.find(v => v.id === selectedId.value) ?? null)

/// Versions grouped by generation, preserving list order (newest-first).
/// Used by the sidebar to render one collapsible section per generation.
const versionsByGroup = computed(() => {
  const groups: Record<string, Version[]> = { Z: [], G: [], Forward: [], Season: [], Other: [] }
  for (const v of versions.value) {
    const key = v.generation ?? 'Other'
    groups[key].push(v)
  }
  return groups
})
const selectedPath = computed(() => selectedId.value ? (installedPaths.value[selectedId.value] ?? '') : '')
const isInstalled = computed(() => !!selectedPath.value)
const isDownloading = computed(() => selectedId.value ? !!downloading.value[selectedId.value] : false)
const isAuthenticated = computed(() => authStep.value === 'done' && !!activeChar.value)

// Quick-play (top-bar Play button).
// Resolves the Version object for lastPlayedId, forgetting it if the user has
// since wiped the install. `quickPlayReady` only turns true once the session
// is also authenticated — otherwise clicking jumps to the Server tab.
const lastPlayed = computed<Version | null>(() => {
  if (!lastPlayedId.value) return null
  const v = versions.value.find(x => x.id === lastPlayedId.value) ?? null
  if (v && !installedPaths.value[v.id]) return null
  return v
})
const quickPlayReady = computed(() =>
  !!lastPlayed.value && isAuthenticated.value && !downloading.value[lastPlayed.value.id]
)

// Welcome is shown when nothing has been installed yet AND the user hasn't
// explicitly dismissed it. Either condition flipping (an install appearing,
// or the user clicking "Browse versions") hides it permanently.
const hasAnyInstall = computed(() => Object.keys(installedPaths.value).length > 0)
const showWelcome = computed(() => !welcomeDismissed.value && !hasAnyInstall.value)
const recommendedVersion = computed(() =>
  versions.value.find(v => v.id === RECOMMENDED_ID) ?? null
)

// ── Lifecycle ────────────────────────────────────────────────────────────────

onMounted(async () => {
  versions.value = await invoke<Version[]>('list_versions')
  if (versions.value.length > 0) selectedId.value = versions.value[0].id

  await listen<ProgressEvent>('download-progress', (event) => {
    const p = event.payload
    const pct = p.bytes_total > 0 ? Math.round((p.bytes_done / p.bytes_total) * 100) : 0
    if (p.phase === 'done' || p.phase === 'error') {
      delete downloading.value[p.version]
      if (p.phase === 'done') showToast(`${p.version} installed successfully!`)
      else showToast(p.message ?? 'Download failed', 'err')
    } else {
      downloading.value[p.version] = { phase: p.phase, progress: pct, message: p.message ?? '' }
    }
  })
})

// ── Actions ───────────────────────────────────────────────────────────────────

async function pickInstallPath() {
  const dir = await open({ directory: true, multiple: false, title: 'Select install folder' })
  if (dir && selectedId.value) {
    installedPaths.value[selectedId.value] = dir as string
    savePaths()
  }
}

/// Welcome-screen "Get started" path: dismiss the screen, jump to the
/// recommended version, and open the install folder picker so the user
/// lands one click away from the download starting.
async function startGuidedSetup() {
  dismissWelcome()
  view.value = 'library'
  if (recommendedVersion.value) {
    selectedId.value = recommendedVersion.value.id
  }
  await startDownload()
}

async function startDownload() {
  if (!selected.value || !selectedId.value) return
  let dest = selectedPath.value
  if (!dest) {
    const dir = await open({ directory: true, multiple: false, title: 'Choose installation folder' })
    if (!dir) return
    dest = dir as string
    installedPaths.value[selectedId.value] = dest
    savePaths()
  }
  try {
    await invoke('download_version', { version: selectedId.value, dest })
  } catch (e) {
    // error already emitted via event
  }
}

async function launchGame() {
  if (!selectedPath.value || !selectedId.value) return

  if (!isAuthenticated.value || !activeChar.value || !authSession.value) {
    showToast('Not authenticated — go to the Server tab first', 'err')
    view.value = 'server'
    return
  }

  saveLastPlayed(selectedId.value)

  try {
    await invoke('launch_game_authed', {
      path: selectedPath.value,
      version: selectedId.value.toUpperCase(),
      server: serverUrl.value,
      sessionJson: authSession.value,
      charId: activeChar.value.id,
    })
  } catch (e: any) {
    showToast(e, 'err')
  }
}

/// Top-bar quick-play: re-select the last-played version so Library shows the
/// right detail pane if the user navigates back, then launch. If auth has
/// expired (session is in-memory only) we jump to Server instead.
async function quickPlay() {
  if (!lastPlayed.value) return
  selectedId.value = lastPlayed.value.id
  if (!isAuthenticated.value || !activeChar.value || !authSession.value) {
    view.value = 'server'
    showToast('Sign in to launch', 'err')
    return
  }
  await launchGame()
}

async function fetchLauncher() {
  if (!selectedPath.value) return
  try {
    await invoke('extract_launcher', { path: selectedPath.value })
    showToast('Launcher binary extracted')
  } catch (e: any) {
    showToast(e, 'err')
  }
}

async function runAvExclude() {
  if (!selectedPath.value) return
  try {
    await invoke('av_exclude', { path: selectedPath.value })
    showToast('AV exclusion applied')
  } catch (e: any) {
    showToast(e, 'err')
  }
}

async function verifyInstall() {
  if (!selectedId.value || !selectedPath.value) return
  const id = selectedId.value
  verifying.value[id] = true
  delete verifyResults.value[id]
  try {
    const result = await invoke<VerifyResult>('verify_version', {
      version: id,
      path: selectedPath.value,
    })
    verifyResults.value[id] = result
    showToast(result.ok ? 'Verification passed ✓' : `Verification failed — ${result.failure_count} issue(s)`, result.ok ? 'ok' : 'err')
  } catch (e: any) {
    showToast(e, 'err')
  } finally {
    verifying.value[id] = false
  }
}

async function runChecks() {
  checksLoading.value = true
  checks.value = []
  try {
    checks.value = await invoke<CheckItem[]>('run_checks', {
      gamePath: checkGamePath.value || null
    })
  } finally {
    checksLoading.value = false
  }
}
</script>

<template>
  <div class="layout">
    <!-- Top bar: brand + global tabs span the whole window -->
    <header class="top-bar">
      <span class="brand">MHF Launcher</span>
      <nav class="nav-tabs">
        <button :class="['nav-tab', view === 'library' ? 'active' : '']" @click="view = 'library'">
          Library
        </button>
        <button :class="['nav-tab', view === 'server' ? 'active' : '']" @click="view = 'server'">
          Server
          <span class="auth-dot" :class="isAuthenticated ? 'ok' : 'off'"></span>
        </button>
        <button :class="['nav-tab', view === 'translations' ? 'active' : '']" @click="view = 'translations'">
          Translations
        </button>
        <button :class="['nav-tab', view === 'checks' ? 'active' : '']" @click="view = 'checks'; runChecks()">
          Checks
        </button>
      </nav>
      <!-- Quick-play: visible once the user has launched at least once -->
      <button
        v-if="lastPlayed"
        class="quick-play"
        :class="{ ready: quickPlayReady }"
        :disabled="!!downloading[lastPlayed.id]"
        :title="quickPlayReady ? `Launch ${lastPlayed.name}` : 'Sign in to launch'"
        @click="quickPlay"
      >
        <span class="qp-icon">&#x25B6;</span>
        <span class="qp-label">Play</span>
        <span class="qp-version">{{ lastPlayed.name }}</span>
      </button>
    </header>

    <!-- First-run welcome screen: takes over the body when no installs exist
         and the user hasn't dismissed it. Shown above all other content. -->
    <Transition name="pane" mode="out-in">
    <section v-if="showWelcome" class="welcome-screen">
      <div class="welcome-card">
        <h1 class="welcome-title">Welcome to MHF Launcher</h1>
        <p class="welcome-tagline">
          Monster Hunter Frontier was Capcom's MMORPG, shut down on
          2019-12-18. This launcher reconnects the original game client
          to a community-run <strong>Erupe</strong> server so the game can be
          played again, in service of preservation.
        </p>

        <ol class="welcome-steps">
          <li>
            <span class="step-num">1</span>
            <div>
              <div class="step-name">Install the game files</div>
              <div class="step-desc">
                Download a verified archive from archive.org. We recommend
                <strong>{{ recommendedVersion?.name ?? 'MHF-ZZ' }}</strong>
                <span v-if="recommendedVersion?.archive_size_gb">
                  ({{ recommendedVersion.archive_size_gb.toFixed(1) }} GB)
                </span>
                — the latest content patch with the most active community.
              </div>
            </div>
          </li>
          <li>
            <span class="step-num">2</span>
            <div>
              <div class="step-name">Sign in to a server</div>
              <div class="step-desc">
                Register a free account on any Erupe-compatible server, then
                pick or create a character. No password is ever stored on disk.
              </div>
            </div>
          </li>
          <li>
            <span class="step-num">3</span>
            <div>
              <div class="step-name">Play</div>
              <div class="step-desc">
                Click Play. After your first launch, a quick-play button stays
                pinned to the top bar so you can jump back in instantly.
              </div>
            </div>
          </li>
        </ol>

        <div class="welcome-actions">
          <button class="btn-primary welcome-cta" @click="startGuidedSetup">
            &#x2B07; Install {{ recommendedVersion?.name ?? 'MHF-ZZ' }}
          </button>
          <button class="btn-outline" @click="dismissWelcome">
            Browse all versions
          </button>
        </div>

        <p class="welcome-note">
          MHF is © Capcom Co., Ltd. and is no longer commercially available.
          You are responsible for compliance with the laws of your country.
        </p>
      </div>
    </section>

    <!-- Body: version sidebar (Library only) + content -->
    <div class="body" v-else>
      <aside class="sidebar" v-if="view === 'library'">
        <div class="version-list">
          <div
            v-for="g in GENERATIONS"
            :key="g.key"
            class="gen-group"
            :class="{ open: openGroups[g.key] }"
            v-show="versionsByGroup[g.key].length > 0"
          >
            <button
              class="gen-summary"
              type="button"
              :aria-expanded="openGroups[g.key]"
              @click="toggleGroup(g.key)"
            >
              <span class="gen-chevron" aria-hidden="true">&#9656;</span>
              <span class="gen-label">{{ g.label }}</span>
              <span class="gen-count">{{ versionsByGroup[g.key].length }}</span>
            </button>
            <Transition
              :css="false"
              @enter="onGroupEnter"
              @leave="onGroupLeave"
            >
              <div v-show="openGroups[g.key]" class="gen-body">
                <button
                  v-for="v in versionsByGroup[g.key]"
                  :key="v.id"
                  :class="[
                    'version-card',
                    v.id === selectedId ? 'selected' : '',
                    installedPaths[v.id] ? 'installed' : '',
                    !v.has_archive ? 'unavailable' : '',
                  ]"
                  @click="selectedId = v.id"
                >
                  <div class="vc-header">
                    <span class="vc-name">{{ v.name }}</span>
                    <span class="vc-badge" v-if="installedPaths[v.id]">&#10003;</span>
                    <span class="vc-badge dl" v-else-if="downloading[v.id]">&#x2193;</span>
                    <span class="vc-badge missing" v-else-if="!v.has_archive" title="No archive source">?</span>
                    <span
                      v-else-if="v.id === RECOMMENDED_ID"
                      class="vc-badge recommended"
                      title="Recommended for new players"
                    >★</span>
                  </div>
                  <div class="vc-meta">
                    <span v-if="v.released">{{ v.released }}</span>
                    <span v-else>{{ v.platform }}</span>
                  </div>
                  <div class="vc-progress" v-if="downloading[v.id]">
                    <div class="vc-progress-bar" :style="{ width: downloading[v.id].progress + '%' }"></div>
                  </div>
                </button>
              </div>
            </Transition>
          </div>
        </div>
      </aside>

      <main class="content">
        <Transition name="pane" mode="out-in">
        <div :key="view" class="pane-slot">

      <!-- Library view: version detail -->
      <div class="detail-pane" v-if="view === 'library' && selected">
        <div class="detail-header">
          <div>
            <h1 class="detail-title">{{ selected.name }}</h1>
            <p class="detail-desc">{{ selected.description }}</p>
            <div class="detail-meta">
              <span class="meta-tag" v-if="selected.released">Released {{ selected.released }}</span>
              <span class="meta-tag">{{ selected.platform }}</span>
              <span
                v-for="code in selected.languages"
                :key="code"
                class="meta-tag lang-tag"
                :title="code"
              >{{ langLabel(code) }}</span>
              <span class="meta-tag" v-if="selected.archive_format">{{ selected.archive_format }}</span>
              <span class="meta-tag" v-if="selected.archive_size_gb">
                {{ selected.archive_size_gb.toFixed(1) }} GB
              </span>
              <span class="meta-tag no-src" v-if="!selected.has_archive">No archive source</span>
            </div>
          </div>
        </div>

        <!-- Changelog / major features -->
        <div class="section" v-if="selected.features.length > 0">
          <label class="section-label">Major features</label>
          <ul class="feature-list">
            <li v-for="(f, i) in selected.features" :key="i">{{ f }}</li>
          </ul>
        </div>

        <!-- Archive-less placeholder banner -->
        <div class="section missing-archive" v-if="!selected.has_archive">
          <div class="check-row warning">
            <div class="check-icon">&#9888;</div>
            <div class="check-body">
              <div class="check-name">No archive source yet</div>
              <div class="check-detail">
                This version is documented but we don't have a downloadable
                archive. If you own a copy of the files, please
                <a href="https://github.com/Mogapedia/mhf-outpost/issues" target="_blank" rel="noopener">open an issue</a>
                so we can add it to archive.org and wire up the manifest.
              </div>
            </div>
          </div>
        </div>

        <!-- Install path (only when an archive exists) -->
        <div class="section" v-if="selected.has_archive">
          <label class="section-label">Install path</label>
          <div class="path-row">
            <input class="path-input" :value="selectedPath" readonly placeholder="Not set — click to choose…" @click="pickInstallPath" />
            <button class="btn-outline" @click="pickInstallPath">Browse…</button>
          </div>
        </div>

        <!-- Download progress -->
        <div class="section" v-if="downloading[selected.id]">
          <div class="progress-block">
            <div class="progress-label">
              {{ downloading[selected.id].phase.toUpperCase() }} — {{ downloading[selected.id].progress }}%
              <span v-if="downloading[selected.id].message"> — {{ downloading[selected.id].message }}</span>
            </div>
            <div class="progress-track">
              <div class="progress-fill" :style="{ width: downloading[selected.id].progress + '%' }"></div>
            </div>
          </div>
        </div>

        <!-- Auth status banner (when installed) -->
        <div class="auth-banner" v-if="isInstalled">
          <template v-if="isAuthenticated">
            <span class="auth-banner-ok">&#10003; Authenticated as <strong>{{ authUsername }}</strong> — <strong>{{ activeChar!.name }}</strong></span>
            <button class="btn-link" @click="view = 'server'">Change…</button>
          </template>
          <template v-else>
            <span class="auth-banner-warn">&#9888; Not authenticated</span>
            <button class="btn-link" @click="view = 'server'">Authenticate →</button>
          </template>
        </div>

        <!-- Action buttons (hidden for archive-less versions that aren't installed) -->
        <div class="actions" v-if="isInstalled || selected.has_archive">
          <template v-if="isInstalled">
            <button
              class="btn-primary"
              :disabled="isDownloading"
              @click="launchGame"
            >&#x25B6;  Play</button>
            <button
              class="btn-outline"
              :disabled="verifying[selected.id]"
              @click="verifyInstall"
            >{{ verifying[selected.id] ? 'Verifying…' : '&#x2713;  Verify' }}</button>
            <button class="btn-outline" @click="fetchLauncher">Update launcher</button>
            <button class="btn-outline" @click="runAvExclude">AV Exclude (Windows)</button>
          </template>
          <template v-else>
            <button
              class="btn-primary"
              :disabled="!selected.has_archive || isDownloading"
              @click="startDownload"
            >
              {{ isDownloading ? 'Downloading…' : selected.has_archive ? '&#x2B07;  Install' : 'No source available' }}
            </button>
            <button class="btn-outline" v-if="selectedPath" @click="fetchLauncher">Get launcher only</button>
          </template>
        </div>

        <!-- Verify result -->
        <div class="verify-result" v-if="selectedId && verifyResults[selectedId]"
             :class="verifyResults[selectedId].ok ? 'verify-ok' : 'verify-fail'">
          <div class="verify-summary">
            <span v-if="verifyResults[selectedId].ok">
              &#x2713; All {{ verifyResults[selectedId].ok_count }} files verified
              <span v-if="verifyResults[selectedId].placeholder_count">
                ({{ verifyResults[selectedId].placeholder_count }} untracked)
              </span>
            </span>
            <span v-else>
              &#x2717; {{ verifyResults[selectedId].failure_count }} issue(s) —
              {{ verifyResults[selectedId].ok_count }} files OK
            </span>
            <span v-if="verifyResults[selectedId].modified_count" class="verify-modified">
              · {{ verifyResults[selectedId].modified_count }} modified (non-core)
            </span>
          </div>
          <ul class="verify-failures" v-if="verifyResults[selectedId].failures.length">
            <li v-for="f in verifyResults[selectedId].failures" :key="f">{{ f }}</li>
          </ul>
        </div>

        <!-- Tip -->
        <div class="info-box" v-if="!isInstalled && selectedPath">
          <strong>Tip:</strong> If you already have the game files at the chosen path,
          use <em>Get launcher only</em> to download mhf-iel and start playing.
        </div>
      </div>

      <!-- Server view -->
      <div class="server-pane" v-if="view === 'server'">
        <h1 class="server-title">Server</h1>

        <!-- Server URL — always visible, independent of game version -->
        <div class="section">
          <label class="section-label">Server URL</label>
          <input
            class="path-input"
            v-model="serverUrl"
            placeholder="http://127.0.0.1:8080"
            @change="saveServerUrl(); resetAuth()"
          />
          <p class="field-hint">Changing the URL will require you to re-authenticate.</p>
        </div>

        <!-- Logged-in state -->
        <div v-if="authStep === 'done' && activeChar" class="session-card">
          <div class="session-info">
            <span class="session-label">Logged in as</span>
            <span class="session-value">{{ authUsername }}</span>
          </div>
          <div class="session-info">
            <span class="session-label">Character</span>
            <span class="session-value char-name-ok">{{ activeChar.name }}
              <span class="char-hr">HR {{ activeChar.hr }} / GR {{ activeChar.gr }}</span>
            </span>
          </div>
          <div class="session-actions">
            <button class="btn-secondary" @click="authStep = 'characters'">Switch character</button>
            <button class="btn-outline" @click="resetAuth">Log out</button>
          </div>
        </div>

        <!-- Character picker (after login, multiple chars) -->
        <div v-else-if="authStep === 'characters'" class="section">
          <label class="section-label">Choose character</label>
          <div class="char-list">
            <button
              v-for="c in authChars"
              :key="c.id"
              class="char-card"
              :disabled="authLoading"
              @click="selectChar(c)"
            >
              <span class="char-name">{{ c.name }}</span>
              <span class="char-meta">HR {{ c.hr }} / GR {{ c.gr }}</span>
            </button>
            <button class="char-card new-char" :disabled="authLoading" @click="createAndSelectChar">
              + New character
            </button>
          </div>
          <div class="auth-error" v-if="authError">{{ authError }}</div>
        </div>

        <!-- Login / register form -->
        <div v-else class="section">
          <div class="field-row">
            <button :class="['tab-btn', authAction === 'login' ? 'active' : '']" @click="authAction = 'login'">Login</button>
            <button :class="['tab-btn', authAction === 'register' ? 'active' : '']" @click="authAction = 'register'">Register</button>
          </div>
          <div class="field">
            <label>Username</label>
            <input v-model="authUsername" type="text" placeholder="your username" autocomplete="username" @keyup.enter="submitCredentials" />
          </div>
          <div class="field">
            <label>Password</label>
            <input v-model="authPassword" type="password" placeholder="••••••••" autocomplete="current-password" @keyup.enter="submitCredentials" />
          </div>
          <div class="auth-error" v-if="authError">{{ authError }}</div>
          <button
            class="btn-primary"
            style="align-self: flex-start"
            :disabled="authLoading || !authUsername || !authPassword"
            @click="submitCredentials"
          >
            {{ authLoading ? 'Connecting…' : authAction === 'login' ? 'Login' : 'Register' }}
          </button>
        </div>
      </div>

      <!-- Translations view -->
      <div class="trans-pane" v-if="view === 'translations'">
        <h1>Translations</h1>
        <p class="trans-desc">
          Download and apply community translations directly to your game
          files. mhf-outpost handles decompression, pointer patching and
          re-encryption in one step — no external tools required.
        </p>

        <div class="section">
          <label class="section-label">Game directory</label>
          <div class="path-row">
            <input class="path-input" :value="selectedPath" readonly
                   :placeholder="selectedPath ? '' : 'Select a version in Library first'" />
          </div>
        </div>

        <div class="section trans-options">
          <div class="trans-field">
            <label class="section-label">Language</label>
            <input class="input" v-model="transLang" placeholder="fr" @change="saveTransPrefs" />
          </div>
        </div>

        <details class="trans-advanced">
          <summary>Advanced</summary>
          <div class="trans-field">
            <label class="section-label">Source repository</label>
            <input class="input" v-model="transRepo" @change="saveTransPrefs" />
            <p class="field-hint">GitHub owner/repo hosting the translation releases. Change only if you're testing a fork.</p>
          </div>
        </details>

        <div class="actions">
          <button class="btn-primary" @click="downloadTranslations"
                  :disabled="transLoading || !selectedPath">
            {{ transLoading ? 'Downloading & applying…' : 'Download & apply translations' }}
          </button>
        </div>

        <div class="trans-result" v-if="transResult">
          <div class="check-row ok">
            <div class="check-icon">&#10003;</div>
            <div class="check-body">
              <div class="check-name">Translations applied</div>
              <div class="check-detail">
                Release <strong>{{ transResult.release_tag }}</strong> — payload saved to <code>{{ transResult.json_path }}</code>
              </div>
            </div>
          </div>
        </div>

        <div class="trans-result" v-if="transError">
          <div class="check-row error">
            <div class="check-icon">&#10007;</div>
            <div class="check-body">
              <div class="check-name">Download failed</div>
              <div class="check-detail">{{ transError }}</div>
            </div>
          </div>
        </div>
      </div>

      <!-- System check view -->
      <div class="checks-pane" v-if="view === 'checks'">
        <div class="checks-header">
          <h1>System Check</h1>
          <div class="check-path-row">
            <input
              class="path-input"
              v-model="checkGamePath"
              placeholder="Optional: game directory path…"
            />
            <button class="btn-primary" @click="runChecks" :disabled="checksLoading">
              {{ checksLoading ? 'Checking…' : 'Run checks' }}
            </button>
          </div>
        </div>

        <div class="check-list" v-if="checks.length > 0">
          <div
            v-for="c in checks"
            :key="c.name"
            :class="['check-row', c.status]"
          >
            <div class="check-icon">
              {{ c.status === 'ok' ? '✓' : c.status === 'warning' ? '⚠' : '✗' }}
            </div>
            <div class="check-body">
              <div class="check-name">{{ c.name }}</div>
              <div class="check-detail">{{ c.detail }}</div>
              <div class="check-fix" v-if="c.fix">→ {{ c.fix }}</div>
            </div>
          </div>
        </div>

        <div class="empty-state" v-else-if="!checksLoading">
          Click "Run checks" to validate your system configuration.
        </div>
      </div>

        </div>
        </Transition>
      </main>
    </div>
    </Transition>

    <!-- Toast -->
    <div :class="['toast', toast?.type]" v-if="toast">{{ toast.text }}</div>
  </div>
</template>

<style scoped>
.layout {
  display: grid;
  grid-template-rows: auto 1fr;
  height: 100vh;
  overflow: hidden;
}

/* ── Top bar ── */
.top-bar {
  display: flex;
  align-items: stretch;
  gap: 32px;
  padding-left: 20px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
}

.brand {
  display: flex;
  align-items: center;
  font-size: 15px;
  font-weight: 700;
  letter-spacing: .04em;
  color: var(--accent);
  text-transform: uppercase;
}

.nav-tabs {
  display: flex;
  align-self: stretch;
}

/* Quick-play button: pushed to the right edge of the top bar. */
.quick-play {
  margin-left: auto;
  align-self: center;
  margin-right: 12px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text-dim);
  font-weight: 700;
  font-size: 13px;
  cursor: pointer;
  transition: background .15s, border-color .15s, color .15s, transform .1s;
}
.quick-play:hover:not(:disabled) { border-color: var(--text-dim); color: var(--text); }
.quick-play:active:not(:disabled) { transform: translateY(1px); }
.quick-play:disabled { opacity: .45; cursor: not-allowed; }

/* Ready (installed + authenticated): promoted to primary CTA. */
.quick-play.ready {
  background: var(--accent);
  border-color: var(--accent);
  color: #1a1400;
}
.quick-play.ready:hover:not(:disabled) {
  background: #e0b850;
  border-color: #e0b850;
  color: #1a1400;
}

.qp-icon    { font-size: 11px; }
.qp-label   { letter-spacing: .04em; text-transform: uppercase; }
.qp-version {
  font-weight: 500;
  opacity: .8;
  padding-left: 8px;
  border-left: 1px solid currentColor;
}

.nav-tab {
  padding: 14px 22px;
  background: transparent;
  color: var(--text-dim);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: .03em;
  text-transform: uppercase;
  transition: color .15s, background .15s;
  position: relative;
}

/* ── Body (sidebar + content) ── */
.body {
  display: flex;
  overflow: hidden;
}

/* ── Sidebar (version list; Library only) ── */
.sidebar {
  width: 240px;
  min-width: 200px;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.nav-tab.active, .nav-tab:hover {
  color: var(--text);
  background: var(--bg-hover);
}

.nav-tab.active {
  border-bottom: 2px solid var(--accent);
}

/* Small status dot on the Server tab */
.auth-dot {
  display: inline-block;
  width: 6px; height: 6px;
  border-radius: 50%;
  margin-left: 4px;
  vertical-align: middle;
  position: relative;
  top: -1px;
}
.auth-dot.ok  { background: var(--ok); }
.auth-dot.off { background: var(--text-dim); }

.version-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.gen-group {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.gen-summary {
  width: 100%;
  background: transparent;
  border: none;
  cursor: pointer;
  padding: 6px 10px;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: .06em;
  text-transform: uppercase;
  color: var(--text-dim);
  display: flex;
  align-items: center;
  gap: 6px;
  border-radius: 4px;
  user-select: none;
  text-align: left;
}
.gen-summary:hover { background: var(--bg-hover); color: var(--text); }
.gen-group.open .gen-summary { color: var(--text); }

.gen-chevron {
  display: inline-block;
  font-size: 9px;
  color: var(--text-dim);
  transition: transform 180ms ease;
}
.gen-group.open .gen-chevron { transform: rotate(90deg); }

.gen-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.gen-count {
  margin-left: auto;
  font-size: 10px;
  font-weight: 600;
  color: var(--text-dim);
  background: var(--bg-card);
  padding: 1px 7px;
  border-radius: 10px;
}

.version-card {
  width: 100%;
  background: var(--bg-card);
  border: 1px solid transparent;
  border-radius: 6px;
  padding: 10px 12px;
  margin-top: 2px;
  text-align: left;
  cursor: pointer;
  transition: background .12s, border-color .12s, opacity .12s;
}

.version-card:hover { background: var(--bg-hover); }
.version-card.selected { border-color: var(--accent); background: var(--bg-hover); }
.version-card.installed .vc-name { color: var(--ok); }

/* Archive-less versions: quieter, smaller, but still clickable for details. */
.version-card.unavailable {
  background: transparent;
  padding: 6px 10px;
  opacity: .55;
}
.version-card.unavailable:hover { opacity: .85; background: var(--bg-hover); }
.version-card.unavailable.selected { opacity: 1; }
.version-card.unavailable .vc-name { font-size: 12px; color: var(--text-dim); font-weight: 500; }
.version-card.unavailable .vc-meta { font-size: 10px; }

.vc-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.vc-name {
  font-weight: 600;
  font-size: 13px;
  color: var(--text);
}

.vc-badge {
  font-size: 11px;
  color: var(--ok);
  font-weight: 700;
}

.vc-badge.dl { color: var(--accent); }
.vc-badge.missing { color: var(--text-dim); font-weight: 600; }

.vc-meta {
  font-size: 11px;
  color: var(--text-dim);
  margin-top: 2px;
}

.vc-progress {
  height: 2px;
  background: var(--border);
  border-radius: 1px;
  margin-top: 6px;
}

.vc-progress-bar {
  height: 100%;
  background: var(--accent);
  border-radius: 1px;
  transition: width .3s;
}

/* ── Content ── */
.content {
  flex: 1;
  overflow-y: auto;
  background: var(--bg-deep);
}

.pane-slot { height: 100%; }

/* Tab switch: short cross-fade (mode="out-in", so old leaves before new enters). */
.pane-enter-active,
.pane-leave-active {
  transition: opacity 120ms ease, transform 120ms ease;
}
.pane-enter-from { opacity: 0; transform: translateY(4px); }
.pane-leave-to   { opacity: 0; transform: translateY(-4px); }

/* ── Detail pane ── */
.detail-pane {
  padding: 32px 40px;
  max-width: 740px;
  display: flex;
  flex-direction: column;
  gap: 28px;
}

.detail-header { display: flex; flex-direction: column; gap: 8px; }

.detail-title {
  font-size: 28px;
  font-weight: 700;
  color: var(--text);
}

.detail-desc {
  color: var(--text-dim);
  font-size: 14px;
  line-height: 1.5;
}

.detail-meta {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 4px;
}

.meta-tag {
  padding: 2px 8px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 12px;
  color: var(--text-dim);
}

.meta-tag.no-src { color: var(--err); border-color: var(--err); }
.meta-tag.lang-tag {
  color: var(--accent);
  border-color: var(--accent);
  font-weight: 700;
  letter-spacing: .04em;
}

.section { display: flex; flex-direction: column; gap: 8px; }

.feature-list {
  margin: 0;
  padding-left: 20px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.feature-list li {
  font-size: 13px;
  line-height: 1.5;
  color: var(--text);
}

.missing-archive .check-detail a {
  color: var(--accent);
  text-decoration: underline;
}

.section-label {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: .06em;
  text-transform: uppercase;
  color: var(--text-dim);
}

.path-row {
  display: flex;
  gap: 8px;
}

.path-input {
  flex: 1;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text);
  padding: 8px 12px;
  outline: none;
  cursor: pointer;
}

.path-input:focus { border-color: var(--accent); }
.input {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text);
  padding: 8px 12px;
  outline: none;
  font-size: 14px;
}
.input:focus { border-color: var(--accent); }

.progress-block { display: flex; flex-direction: column; gap: 6px; }

.progress-label {
  font-size: 12px;
  color: var(--accent);
  font-weight: 600;
}

.progress-track {
  height: 6px;
  background: var(--bg-card);
  border-radius: 3px;
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
  transition: width .4s;
}

/* Auth banner in Library view */
.auth-banner {
  display: flex;
  align-items: center;
  gap: 10px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 10px 14px;
  font-size: 13px;
}
.auth-banner-ok  { color: var(--ok); flex: 1; }
.auth-banner-warn { color: var(--warn); flex: 1; }

.btn-link {
  background: none;
  color: var(--accent);
  font-size: 12px;
  font-weight: 600;
  padding: 0;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}

.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}

.btn-primary {
  background: var(--accent);
  color: #1a1400;
  font-weight: 700;
  padding: 10px 24px;
  border-radius: 6px;
  font-size: 14px;
  transition: background .15s;
}

.btn-primary:hover:not(:disabled) { background: #e0b850; }
.btn-primary:disabled { opacity: .45; cursor: not-allowed; }

.btn-secondary {
  background: transparent;
  border: 1px solid var(--accent);
  color: var(--accent);
  padding: 10px 20px;
  border-radius: 6px;
  font-weight: 600;
  transition: background .15s;
}

.btn-secondary:hover { background: rgba(201,168,76,.1); }

.btn-outline {
  background: var(--bg-card);
  border: 1px solid var(--border);
  color: var(--text-dim);
  padding: 10px 16px;
  border-radius: 6px;
  transition: border-color .15s, color .15s;
}

.btn-outline:hover { border-color: var(--text-dim); color: var(--text); }

.info-box {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-left: 3px solid var(--accent);
  padding: 12px 16px;
  border-radius: 6px;
  font-size: 13px;
  color: var(--text-dim);
  line-height: 1.6;
}

.verify-result {
  border-radius: 6px;
  border: 1px solid var(--border);
  padding: 12px 16px;
  font-size: 13px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.verify-ok  { border-left: 3px solid var(--ok);  background: rgba(86,168,86,.06); }
.verify-fail { border-left: 3px solid var(--err); background: rgba(207,79,79,.06); }
.verify-summary { font-weight: 600; }
.verify-ok .verify-summary  { color: var(--ok); }
.verify-fail .verify-summary { color: var(--err); }
.verify-modified { color: var(--warn); font-weight: 400; }
.verify-failures {
  margin: 0; padding: 0 0 0 16px;
  font-size: 12px; color: var(--err);
  display: flex; flex-direction: column; gap: 2px;
}

/* ── Server pane ── */
.server-pane {
  padding: 32px 40px;
  max-width: 560px;
  display: flex;
  flex-direction: column;
  gap: 28px;
}

.server-title {
  font-size: 28px;
  font-weight: 700;
  color: var(--text);
}

.field-hint {
  font-size: 12px;
  color: var(--text-dim);
  margin: 0;
}

/* Session card (logged-in state) */
.session-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-left: 3px solid var(--ok);
  border-radius: 8px;
  padding: 16px 20px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.session-info { display: flex; flex-direction: column; gap: 2px; }
.session-label { font-size: 11px; text-transform: uppercase; letter-spacing: .05em; color: var(--text-dim); font-weight: 700; }
.session-value { font-size: 14px; color: var(--text); font-weight: 600; }
.char-name-ok { color: var(--ok); }
.char-hr { font-size: 12px; color: var(--text-dim); font-weight: 400; margin-left: 8px; }

.session-actions { display: flex; gap: 8px; margin-top: 4px; }

/* Auth form fields */
.field-row { display: flex; gap: 6px; }

.tab-btn {
  flex: 1; padding: 7px 0;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 5px;
  color: var(--text-dim);
  font-weight: 600; font-size: 13px;
  cursor: pointer; transition: all .12s;
}
.tab-btn.active { border-color: var(--accent); color: var(--accent); background: rgba(201,168,76,.08); }
.tab-btn:hover:not(.active) { color: var(--text); }

.field { display: flex; flex-direction: column; gap: 5px; }
.field label { font-size: 11px; font-weight: 700; letter-spacing: .05em; text-transform: uppercase; color: var(--text-dim); }
.field input {
  background: var(--bg-card); border: 1px solid var(--border);
  border-radius: 5px; color: var(--text);
  padding: 8px 10px; outline: none;
}
.field input:focus { border-color: var(--accent); }

.auth-error { font-size: 12px; color: var(--err); background: rgba(207,79,79,.1); border: 1px solid var(--err); border-radius: 5px; padding: 8px 10px; }

/* Character list (in Server tab) */
.char-list { display: flex; flex-direction: column; gap: 6px; }

.char-card {
  display: flex; align-items: center; justify-content: space-between;
  background: var(--bg-card); border: 1px solid var(--border);
  border-radius: 6px; padding: 10px 14px;
  cursor: pointer; text-align: left; width: 100%;
  transition: border-color .12s, background .12s;
}
.char-card:hover:not(:disabled) { border-color: var(--accent); background: var(--bg-hover); }
.char-card:disabled { opacity: .5; cursor: not-allowed; }
.char-card.new-char { color: var(--accent); border-style: dashed; justify-content: center; font-weight: 600; }

.char-name { font-weight: 600; font-size: 13px; }
.char-meta { font-size: 11px; color: var(--text-dim); }

/* ── Checks pane ── */
/* ── Translations pane ──────────────────────────────────────────────────── */

.trans-pane {
  padding: 32px 40px;
  max-width: 640px;
  display: flex;
  flex-direction: column;
  gap: 20px;
}
.trans-desc {
  color: var(--text-dim);
  font-size: 14px;
  line-height: 1.5;
  margin: 0;
}
.trans-options {
  display: flex;
  gap: 16px;
}
.trans-field {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.trans-advanced {
  border-top: 1px solid var(--border);
  padding-top: 12px;
}
.trans-advanced summary {
  cursor: pointer;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-dim);
  text-transform: uppercase;
  letter-spacing: .03em;
  padding: 4px 0;
}
.trans-advanced summary:hover { color: var(--text); }
.trans-advanced .trans-field {
  margin-top: 12px;
}
.trans-result {
  margin-top: 8px;
}
.trans-result code {
  display: block;
  margin-top: 8px;
  padding: 8px 12px;
  background: var(--bg-card);
  border-radius: 6px;
  font-size: 12px;
  word-break: break-all;
  white-space: pre-wrap;
  color: var(--text);
}

.checks-pane {
  padding: 32px 40px;
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.checks-header { display: flex; flex-direction: column; gap: 14px; }
.checks-header h1 { font-size: 24px; font-weight: 700; }

.check-path-row {
  display: flex;
  gap: 10px;
}

.check-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.check-row {
  display: flex;
  gap: 12px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px 16px;
}

.check-row.ok    { border-left: 3px solid var(--ok); }
.check-row.warning { border-left: 3px solid var(--warn); }
.check-row.error { border-left: 3px solid var(--err); }

.check-icon {
  font-size: 16px;
  width: 20px;
  text-align: center;
  margin-top: 1px;
}

.check-row.ok    .check-icon { color: var(--ok); }
.check-row.warning .check-icon { color: var(--warn); }
.check-row.error .check-icon { color: var(--err); }

.check-body { flex: 1; }
.check-name { font-weight: 600; font-size: 13px; }
.check-detail { font-size: 12px; color: var(--text-dim); margin-top: 2px; }
.check-fix {
  font-size: 12px;
  color: var(--accent);
  margin-top: 6px;
  white-space: pre-line;
}

.empty-state {
  color: var(--text-dim);
  font-size: 14px;
  text-align: center;
  padding: 60px 0;
}

/* ── Toast ── */
.toast {
  position: fixed;
  bottom: 20px;
  right: 20px;
  padding: 10px 18px;
  border-radius: 8px;
  font-size: 13px;
  font-weight: 600;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  color: var(--text);
  box-shadow: 0 4px 16px rgba(0,0,0,.4);
  z-index: 1000;
  animation: fadein .2s ease;
}

.toast.ok  { border-color: var(--ok); }
.toast.err { border-color: var(--err); color: var(--err); }

@keyframes fadein { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: none; } }

/* ── Welcome screen ── */
.welcome-screen {
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding: 48px 24px;
  overflow-y: auto;
  background: var(--bg-deep);
}

.welcome-card {
  width: 100%;
  max-width: 640px;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 32px 36px;
  box-shadow: 0 8px 32px rgba(0,0,0,.4);
}

.welcome-title {
  font-size: 22px;
  font-weight: 700;
  color: var(--accent);
  letter-spacing: .02em;
  margin-bottom: 10px;
}

.welcome-tagline {
  font-size: 13px;
  line-height: 1.55;
  color: var(--text-dim);
  margin-bottom: 24px;
}
.welcome-tagline strong { color: var(--text); font-weight: 600; }

.welcome-steps {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin-bottom: 28px;
  padding: 0;
}
.welcome-steps li {
  display: flex;
  gap: 14px;
  align-items: flex-start;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 14px 16px;
}

.step-num {
  flex: 0 0 28px;
  width: 28px;
  height: 28px;
  border-radius: 50%;
  background: var(--accent);
  color: #1a1400;
  font-weight: 700;
  font-size: 13px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.step-name { font-weight: 600; font-size: 13px; margin-bottom: 2px; }
.step-desc { font-size: 12px; color: var(--text-dim); line-height: 1.5; }
.step-desc strong { color: var(--text); font-weight: 600; }

.welcome-actions {
  display: flex;
  gap: 10px;
  margin-bottom: 16px;
}
.welcome-cta { font-size: 14px; padding: 10px 18px; }

.welcome-note {
  font-size: 11px;
  color: var(--text-dim);
  line-height: 1.5;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

/* Recommended badge in the sidebar version cards. */
.vc-badge.recommended {
  color: var(--accent);
  font-size: 13px;
}
</style>
