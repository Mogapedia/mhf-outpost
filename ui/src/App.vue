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
  has_archive: boolean
  archive_size_gb: number | null
  archive_format: string | null
}

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

// ── State ────────────────────────────────────────────────────────────────────

const versions = ref<Version[]>([])
const selectedId = ref<string | null>(null)
const view = ref<'library' | 'checks' | 'settings'>('library')

// Per-version installed paths (stored in localStorage)
const installedPaths = ref<Record<string, string>>(
  JSON.parse(localStorage.getItem('installedPaths') || '{}')
)
function savePaths() {
  localStorage.setItem('installedPaths', JSON.stringify(installedPaths.value))
}

// System checks
const checks = ref<CheckItem[]>([])
const checksLoading = ref(false)
const checkGamePath = ref('')

// Download state
const downloading = ref<Record<string, { phase: string; progress: number; message: string }>>({})

// Per-version server URL (stored in localStorage)
const serverUrls = ref<Record<string, string>>(
  JSON.parse(localStorage.getItem('serverUrls') || '{}')
)
function saveServerUrls() {
  localStorage.setItem('serverUrls', JSON.stringify(serverUrls.value))
}
const selectedServerUrl = computed({
  get: () => selectedId.value ? (serverUrls.value[selectedId.value] ?? 'http://127.0.0.1:8080') : '',
  set: (v: string) => {
    if (selectedId.value) { serverUrls.value[selectedId.value] = v; saveServerUrls() }
  }
})

// Auth modal state
interface CharDto { id: number; name: string; hr: number; gr: number; is_female: boolean }
const authModal = ref(false)
const authStep = ref<'credentials' | 'characters'>('credentials')
const authAction = ref<'login' | 'register'>('login')
const authUsername = ref('')
const authPassword = ref('')
const authLoading = ref(false)
const authError = ref('')
const authSession = ref('')      // opaque JSON blob from backend
const authChars = ref<CharDto[]>([])

function openAuthModal() {
  authModal.value = true
  authStep.value = 'credentials'
  authAction.value = 'login'
  authUsername.value = ''
  authPassword.value = ''
  authError.value = ''
  authSession.value = ''
  authChars.value = []
}

async function submitCredentials() {
  if (!selectedPath.value || !selectedId.value) return
  authLoading.value = true
  authError.value = ''
  try {
    const result = await invoke<{ characters: CharDto[]; session_json: string }>('authenticate', {
      server: selectedServerUrl.value,
      username: authUsername.value,
      password: authPassword.value,
      action: authAction.value,
    })
    authSession.value = result.session_json
    authChars.value = result.characters
    if (result.characters.length === 1) {
      // Auto-select the only character
      await finishAuth(result.characters[0].id)
    } else if (result.characters.length === 0) {
      // Prompt to create first character
      await finishAuth(0)
    } else {
      authStep.value = 'characters'
    }
  } catch (e: any) {
    authError.value = e
  } finally {
    authLoading.value = false
  }
}

async function finishAuth(charId: number) {
  if (!selectedPath.value || !selectedId.value) return
  authLoading.value = true
  authError.value = ''
  try {
    await invoke('select_character', {
      gamePath: selectedPath.value,
      server: selectedServerUrl.value,
      sessionJson: authSession.value,
      charId,
      version: selectedId.value.toUpperCase(),
    })
    authModal.value = false
    showToast('Authenticated — config.json saved')
  } catch (e: any) {
    authError.value = e
  } finally {
    authLoading.value = false
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
const selectedPath = computed(() => selectedId.value ? (installedPaths.value[selectedId.value] ?? '') : '')
const isInstalled = computed(() => !!selectedPath.value)
const isDownloading = computed(() => selectedId.value ? !!downloading.value[selectedId.value] : false)

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

async function launchGame(auth = false) {
  if (!selectedPath.value) return
  try {
    await invoke('launch_game', { path: selectedPath.value, auth })
  } catch (e: any) {
    showToast(e, 'err')
  }
}

async function fetchLauncher() {
  if (!selectedPath.value) return
  try {
    await invoke('fetch_launcher', { path: selectedPath.value })
    showToast('Launcher binaries downloaded')
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
    <!-- Sidebar -->
    <aside class="sidebar">
      <div class="sidebar-header">
        <span class="brand">MHF Launcher</span>
      </div>

      <nav class="nav-tabs">
        <button :class="['nav-tab', view === 'library' ? 'active' : '']" @click="view = 'library'">
          Library
        </button>
        <button :class="['nav-tab', view === 'checks' ? 'active' : '']" @click="view = 'checks'; runChecks()">
          System Check
        </button>
      </nav>

      <div class="version-list" v-if="view === 'library'">
        <button
          v-for="v in versions"
          :key="v.id"
          :class="['version-card', v.id === selectedId ? 'selected' : '', installedPaths[v.id] ? 'installed' : '']"
          @click="selectedId = v.id"
        >
          <div class="vc-header">
            <span class="vc-name">{{ v.name }}</span>
            <span class="vc-badge" v-if="installedPaths[v.id]">&#10003;</span>
            <span class="vc-badge dl" v-else-if="downloading[v.id]">&#x2193;</span>
          </div>
          <div class="vc-platform">{{ v.platform }}</div>
          <div class="vc-progress" v-if="downloading[v.id]">
            <div class="vc-progress-bar" :style="{ width: downloading[v.id].progress + '%' }"></div>
          </div>
        </button>
      </div>
    </aside>

    <!-- Main content -->
    <main class="content">

      <!-- Library view: version detail -->
      <div class="detail-pane" v-if="view === 'library' && selected">
        <div class="detail-header">
          <div>
            <h1 class="detail-title">{{ selected.name }}</h1>
            <p class="detail-desc">{{ selected.description }}</p>
            <div class="detail-meta">
              <span class="meta-tag">{{ selected.platform }}</span>
              <span class="meta-tag" v-if="selected.archive_format">{{ selected.archive_format }}</span>
              <span class="meta-tag" v-if="selected.archive_size_gb">
                {{ selected.archive_size_gb.toFixed(1) }} GB
              </span>
              <span class="meta-tag no-src" v-if="!selected.has_archive">No archive source</span>
            </div>
          </div>
        </div>

        <!-- Install path -->
        <div class="section">
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

        <!-- Server URL (shown when a path is set) -->
        <div class="section" v-if="selectedPath">
          <label class="section-label">Server URL</label>
          <input class="path-input" v-model="selectedServerUrl" placeholder="http://127.0.0.1:8080" />
        </div>

        <!-- Action buttons -->
        <div class="actions">
          <template v-if="isInstalled">
            <button class="btn-primary" @click="launchGame(false)">&#x25B6;  Play</button>
            <button class="btn-secondary" @click="openAuthModal">Authenticate</button>
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

        <!-- If already has path but not fully installed, show fetch launcher tip -->
        <div class="info-box" v-if="!isInstalled && selectedPath">
          <strong>Tip:</strong> If you already have the game files at the chosen path,
          use <em>Get launcher only</em> to download mhf-iel and start playing.
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

    </main>

    <!-- Auth modal -->
    <div class="modal-backdrop" v-if="authModal" @click.self="authModal = false">
      <div class="modal">
        <div class="modal-header">
          <span>{{ authStep === 'credentials' ? 'Authenticate' : 'Select character' }}</span>
          <button class="modal-close" @click="authModal = false">&#x2715;</button>
        </div>

        <!-- Step 1: credentials -->
        <div v-if="authStep === 'credentials'" class="modal-body">
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
          <div class="modal-actions">
            <button class="btn-primary" :disabled="authLoading || !authUsername || !authPassword" @click="submitCredentials">
              {{ authLoading ? 'Connecting…' : authAction === 'login' ? 'Login' : 'Register' }}
            </button>
          </div>
        </div>

        <!-- Step 2: character selection -->
        <div v-if="authStep === 'characters'" class="modal-body">
          <p class="char-hint">Choose a character to play as:</p>
          <div class="char-list">
            <button
              v-for="c in authChars"
              :key="c.id"
              class="char-card"
              :disabled="authLoading"
              @click="finishAuth(c.id)"
            >
              <span class="char-name">{{ c.name }}</span>
              <span class="char-meta">HR {{ c.hr }} / GR {{ c.gr }}</span>
            </button>
            <button class="char-card new-char" :disabled="authLoading" @click="finishAuth(0)">
              + New character
            </button>
          </div>
          <div class="auth-error" v-if="authError">{{ authError }}</div>
        </div>
      </div>
    </div>

    <!-- Toast -->
    <div :class="['toast', toast?.type]" v-if="toast">{{ toast.text }}</div>
  </div>
</template>

<style scoped>
.layout {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

/* ── Sidebar ── */
.sidebar {
  width: 240px;
  min-width: 200px;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.sidebar-header {
  padding: 18px 16px 12px;
  border-bottom: 1px solid var(--border);
}

.brand {
  font-size: 15px;
  font-weight: 700;
  letter-spacing: .04em;
  color: var(--accent);
  text-transform: uppercase;
}

.nav-tabs {
  display: flex;
  border-bottom: 1px solid var(--border);
}

.nav-tab {
  flex: 1;
  padding: 9px 0;
  background: transparent;
  color: var(--text-dim);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: .03em;
  text-transform: uppercase;
  transition: color .15s, background .15s;
}

.nav-tab.active, .nav-tab:hover {
  color: var(--text);
  background: var(--bg-hover);
}

.nav-tab.active {
  border-bottom: 2px solid var(--accent);
}

.version-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.version-card {
  width: 100%;
  background: var(--bg-card);
  border: 1px solid transparent;
  border-radius: 6px;
  padding: 10px 12px;
  text-align: left;
  cursor: pointer;
  transition: background .12s, border-color .12s;
}

.version-card:hover { background: var(--bg-hover); }
.version-card.selected { border-color: var(--accent); background: var(--bg-hover); }
.version-card.installed .vc-name { color: var(--ok); }

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

.vc-platform {
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

.section { display: flex; flex-direction: column; gap: 8px; }

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

/* ── Checks pane ── */
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

/* ── Auth modal ── */
.modal-backdrop {
  position: fixed; inset: 0;
  background: rgba(0,0,0,.6);
  display: flex; align-items: center; justify-content: center;
  z-index: 200;
}

.modal {
  background: var(--bg-panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  width: 380px;
  box-shadow: 0 8px 40px rgba(0,0,0,.5);
  animation: fadein .15s ease;
}

.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
  font-weight: 700;
  font-size: 15px;
}

.modal-close {
  background: none; color: var(--text-dim); font-size: 16px;
  cursor: pointer; transition: color .12s;
}
.modal-close:hover { color: var(--text); }

.modal-body { padding: 20px; display: flex; flex-direction: column; gap: 14px; }

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

.modal-actions { display: flex; justify-content: flex-end; padding-top: 4px; }

.char-hint { font-size: 13px; color: var(--text-dim); }

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
</style>
