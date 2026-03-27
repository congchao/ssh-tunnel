<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "ant-design-vue";
import {
  CloseOutlined,
  SaveOutlined,
  SettingOutlined,
} from "@ant-design/icons-vue";
import playIcon from "./assets/play.svg";
import stopIcon from "./assets/stop.svg";

type SshConfig = {
  host: string;
  port: number;
  username: string;
  password: string;
};

type PortMapping = {
  remoteHost: string;
  remotePort: number;
  localHost: string;
  localPort: number;
  remark: string;
};

type TunnelConfig = {
  ssh: SshConfig;
  mappings: PortMapping[];
};

type UiMapping = PortMapping & { id: string; lastRemotePort?: number };

type LogEvent = {
  level: string;
  message: string;
};

type LogItem = LogEvent & { at: string };
type StatusEvent = { running: boolean; connected: boolean };

type Locale = "zh" | "en";

const defaultConfig: TunnelConfig = {
  ssh: {
    host: "mnet.ds-int.cn",
    port: 56000,
    username: "",
    password: "..",
  },
  mappings: [
    {
      remoteHost: "mysql8-proxy.auto.ds-int.cn",
      remotePort: 6428,
      localHost: "0.0.0.0",
      localPort: 6428,
      remark: "",
    },
    {
      remoteHost: "redis-proxy.radar.ds-int.cn",
      remotePort: 7031,
      localHost: "0.0.0.0",
      localPort: 7031,
      remark: "",
    },
    {
      remoteHost: "n1.auto.es.hdp",
      remotePort: 9256,
      localHost: "0.0.0.0",
      localPort: 9256,
      remark: "",
    },
    {
      remoteHost: "172.16.40.120",
      remotePort: 9250,
      localHost: "0.0.0.0",
      localPort: 9250,
      remark: "",
    },
    {
      remoteHost: "172.16.40.254",
      remotePort: 443,
      localHost: "0.0.0.0",
      localPort: 443,
      remark: "",
    },
    {
      remoteHost: "172.21.17.120",
      remotePort: 35432,
      localHost: "0.0.0.0",
      localPort: 35432,
      remark: "",
    },
    {
      remoteHost: "172.21.22.20",
      remotePort: 30364,
      localHost: "0.0.0.0",
      localPort: 30364,
      remark: "",
    },
    {
      remoteHost: "172.21.22.20",
      remotePort: 32270,
      localHost: "0.0.0.0",
      localPort: 32270,
      remark: "",
    },
    {
      remoteHost: "172.21.22.20",
      remotePort: 32476,
      localHost: "0.0.0.0",
      localPort: 32476,
      remark: "",
    },
    {
      remoteHost: "pg-proxy.auto.ds-int.cn",
      remotePort: 5408,
      localHost: "0.0.0.0",
      localPort: 5408,
      remark: "",
    },
  ],
};

const ssh = ref<SshConfig>({ ...defaultConfig.ssh });
const mappings = ref<UiMapping[]>([]);
const running = ref(false);
const connected = ref(true);
const logs = ref<LogItem[]>([]);
const saving = ref(false);
const loading = ref(true);
const showConfig = ref(false);
const actionLoading = ref(false);
const tableScrollY = ref(240);
const mappingWrapRef = ref<HTMLElement | null>(null);
let mappingObserver: ResizeObserver | null = null;
const locale = ref<Locale>("en");
const localHostOptions = [
  { value: "0.0.0.0" },
  { value: "127.0.0.1" },
];

const statusColor = computed(() => {
  if (!running.value) return "default";
  return connected.value ? "green" : "gold";
});
const messages = {
  zh: {
    brand: "SSH 隧道",
    running: "运行中",
    reconnecting: "重连中",
    stopped: "已停止",
    logsEmpty: "暂无日志",
    settings: "配置中心",
    escClose: "ESC 关闭",
    saveConfig: "保存配置",
    addMapping: "添加映射",
    delete: "删除",
    sshServer: "SSH 服务器",
    mappings: "端口映射列表",
    remoteHost: "远程地址",
    remotePort: "远程端口",
    localHost: "本地监听地址",
    localPort: "本地端口",
    remark: "备注",
    action: "操作",
    host: "主机",
    port: "端口",
    username: "用户名",
    password: "密码",
    hint:
      "本地监听地址建议使用 127.0.0.1（仅本机）或 0.0.0.0（局域网访问）。",
    placeholderRemoteHost: "remote.host",
    placeholderHost: "mnet.ds-int.cn",
    placeholderUser: "user",
    placeholderRemark: "备注",
    saveSuccess: "配置已保存到本地数据库",
    saveFail: "保存失败",
    startFail: "启动失败",
    stopFail: "停止失败",
    language: "语言",
  },
  en: {
    brand: "SSH Tunnel",
    running: "Running",
    reconnecting: "Reconnecting",
    stopped: "Stopped",
    logsEmpty: "No logs yet",
    settings: "Settings",
    escClose: "ESC to close",
    saveConfig: "Save",
    addMapping: "Add Mapping",
    delete: "Delete",
    sshServer: "SSH Server",
    mappings: "Port Mappings",
    remoteHost: "Remote Host",
    remotePort: "Remote Port",
    localHost: "Local Bind",
    localPort: "Local Port",
    remark: "Remark",
    action: "Actions",
    host: "Host",
    port: "Port",
    username: "Username",
    password: "Password",
    hint: "Use 127.0.0.1 (local only) or 0.0.0.0 (LAN access).",
    placeholderRemoteHost: "remote.host",
    placeholderHost: "mnet.ds-int.cn",
    placeholderUser: "user",
    placeholderRemark: "Remark",
    saveSuccess: "Config saved",
    saveFail: "Save failed",
    startFail: "Start failed",
    stopFail: "Stop failed",
    language: "Language",
  },
} as const;

function t(key: keyof (typeof messages)["en"]) {
  return messages[locale.value][key] ?? messages.en[key];
}

function detectLocale(): Locale {
  if (typeof navigator === "undefined") return "en";
  const lang =
    navigator.language ||
    (Array.isArray(navigator.languages) ? navigator.languages[0] : "");
  if (!lang) return "en";
  return lang.toLowerCase().startsWith("zh") ? "zh" : "en";
}

const statusText = computed(() => {
  if (!running.value) return t("stopped");
  return connected.value ? t("running") : t("reconnecting");
});

const canStart = computed(() => {
  if (running.value) return false;
  if (!ssh.value.host.trim() || !ssh.value.username.trim()) return false;
  return mappings.value.length > 0 && mappings.value.every((item) => {
    return (
      item.remoteHost.trim().length > 0 &&
      item.remotePort > 0 &&
      item.localPort > 0
    );
  });
});

const columns = computed(() => [
  { title: t("remoteHost"), dataIndex: "remoteHost", key: "remoteHost", width: 200 },
  { title: t("remotePort"), dataIndex: "remotePort", key: "remotePort",width: 100 },
  { title: t("localHost"), dataIndex: "localHost", key: "localHost",width: 120 },
  { title: t("localPort"), dataIndex: "localPort", key: "localPort",width: 100  },
  { title: t("remark"), dataIndex: "remark", key: "remark" },
  { title: t("action"), key: "actions", align: "center", width: 80 },
]);

function makeId() {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function mapToUi(config: TunnelConfig): UiMapping[] {
  return config.mappings.map((mapping) => ({
    ...mapping,
    remark: mapping.remark ?? "",
    id: makeId(),
    lastRemotePort: mapping.remotePort,
  }));
}

function mapToConfig(): TunnelConfig {
  return {
    ssh: { ...ssh.value },
    mappings: mappings.value.map(({ id, lastRemotePort, ...rest }) => rest),
  };
}

function resetToDefault() {
  ssh.value = { ...defaultConfig.ssh };
  mappings.value = mapToUi(defaultConfig);
}

function addMapping() {
  mappings.value.push({
    id: makeId(),
    remoteHost: "",
    remotePort: 22,
    localHost: "0.0.0.0",
    localPort: 22,
    remark: "",
    lastRemotePort: 22,
  });
}

function removeMapping(id: string) {
  mappings.value = mappings.value.filter((item) => item.id !== id);
}

async function saveConfig() {
  saving.value = true;
  try {
    await invoke("save_config", { config: mapToConfig() });
    message.success(t("saveSuccess"));
  } catch (error) {
    message.error(`${t("saveFail")}: ${String(error)}`);
  } finally {
    saving.value = false;
  }
}

async function startTunnel() {
  if (!canStart.value || actionLoading.value) return;
  actionLoading.value = true;
  try {
    await invoke("save_config", { config: mapToConfig() });
    await invoke("start_tunnel", { config: mapToConfig() });
    running.value = true;
    connected.value = true;
  } catch (error) {
    message.error(`${t("startFail")}: ${String(error)}`);
  } finally {
    actionLoading.value = false;
  }
}

async function stopTunnel() {
  if (actionLoading.value) return;
  actionLoading.value = true;
  try {
    await invoke("stop_tunnel");
    running.value = false;
    connected.value = false;
  } catch (error) {
    message.error(`${t("stopFail")}: ${String(error)}`);
  } finally {
    actionLoading.value = false;
  }
}

async function toggleTunnel() {
  if (running.value) {
    await stopTunnel();
  } else {
    await startTunnel();
  }
}

function onRemotePortChange(record: UiMapping, value: number | null) {
  if (!value) return;
  const prev = record.lastRemotePort ?? record.remotePort;
  record.remotePort = value;
  if (!record.localPort || record.localPort === prev) {
    record.localPort = value;
  }
  record.lastRemotePort = value;
}

function openConfig() {
  showConfig.value = true;
}

function closeConfig() {
  showConfig.value = false;
}

function setupMappingObserver() {
  if (!mappingWrapRef.value) return;
  mappingObserver?.disconnect();
  mappingObserver = new ResizeObserver(() => {
    const wrap = mappingWrapRef.value;
    if (!wrap) return;
    const hint = wrap.querySelector(".hint") as HTMLElement | null;
    const hintHeight = hint?.offsetHeight ?? 0;
    const padding = 8;
    const height = Math.max(160, wrap.clientHeight - hintHeight - padding);
    tableScrollY.value = height;
  });
  mappingObserver.observe(mappingWrapRef.value);
}

let unlisten: (() => void) | null = null;
let unlistenStatus: (() => void) | null = null;
let keyListener: ((event: KeyboardEvent) => void) | null = null;

onMounted(async () => {
  locale.value = detectLocale();
  running.value = await invoke("tunnel_status");
  connected.value = running.value;
  const loaded = await invoke<TunnelConfig | null>("load_config");
  if (loaded) {
    ssh.value = { ...defaultConfig.ssh, ...loaded.ssh };
    mappings.value = mapToUi(loaded);
  } else {
    resetToDefault();
  }
  loading.value = false;
  unlisten = await listen<LogEvent>("tunnel-log", (event) => {
    logs.value.unshift({
      ...event.payload,
      at: new Date().toLocaleTimeString(),
    });
  });
  unlistenStatus = await listen<StatusEvent>("tunnel-status", (event) => {
    running.value = event.payload.running;
    connected.value = event.payload.connected;
  });
  keyListener = (event: KeyboardEvent) => {
    if (event.key === "Escape" && showConfig.value) {
      showConfig.value = false;
    }
  };
  window.addEventListener("keydown", keyListener);
});

watch(showConfig, async (value) => {
  if (value) {
    await nextTick();
    setupMappingObserver();
  }
});

onBeforeUnmount(() => {
  unlisten?.();
  unlistenStatus?.();
  if (keyListener) {
    window.removeEventListener("keydown", keyListener);
  }
  mappingObserver?.disconnect();
});
</script>

<template>
  <div class="app">
    <div class="topbar">
      <div class="brand">
        <span class="brand-title">{{ t("brand") }}</span>
        <a-tag :color="statusColor">{{ statusText }}</a-tag>
      </div>
      <div class="actions">
        <a-select
          v-model:value="locale"
          class="lang-select"
          :options="[
            { value: 'zh', label: '中文' },
            { value: 'en', label: 'English' }
          ]"
        />
        <a-button shape="circle" :loading="actionLoading" @click="toggleTunnel">
          <template v-slot:icon>
            <img
                class="icon-img"
                :src="running? stopIcon :playIcon"
                alt="start"
            />
          </template>
        </a-button>
        <a-button shape="circle" @click="openConfig" aria-label="open config">
          <template v-slot:icon>
            <SettingOutlined />
          </template>
        </a-button>
      </div>
    </div>

    <div class="log-panel">
      <div v-if="logs.length === 0" class="empty">{{ t("logsEmpty") }}</div>
      <a-list v-else :data-source="logs" size="small" class="log-list">
        <template #renderItem="{ item }">
          <a-list-item>
            <a-space>
              <span class="time">{{ item.at }}</span>
              <a-tag
                :color="item.level === 'error' ? 'red' : item.level === 'warn' ? 'gold' : 'green'"
              >
                {{ item.level.toUpperCase() }}
              </a-tag>
              <span>{{ item.message }}</span>
            </a-space>
          </a-list-item>
        </template>
      </a-list>
    </div>

    <div v-if="showConfig" class="config-mask">
      <div class="config-panel">
        <div class="config-header">
          <div class="config-title">
            <span>{{ t("settings") }}</span>
          </div>
          <div class="config-actions">
            <a-button type="primary" :loading="saving" @click="saveConfig">
              <template #icon>
                <SaveOutlined />
              </template>
              {{ t("saveConfig") }}
            </a-button>
            <a-button shape="circle" @click="closeConfig" aria-label="close config">
              <template v-slot:icon>
                <CloseOutlined />
              </template>
            </a-button>
          </div>
        </div>

        <div class="config-body">
          <a-card :loading="loading" :title="t('sshServer')">
            <a-form layout="inline" class="ssh-form">
              <a-form-item :label="t('host')">
                <a-input
                  v-model:value="ssh.host"
                  :placeholder="t('placeholderHost')"
                  style="width: 150px"
                />
              </a-form-item>
              <a-form-item :label="t('port')">
                <a-input-number
                  v-model:value="ssh.port"
                  :min="1"
                  :max="65535"
                  style="width: 80px"
                />
              </a-form-item>
              <a-form-item :label="t('username')">
                <a-input
                  v-model:value="ssh.username"
                  :placeholder="t('placeholderUser')"
                  style="width: 120px"
                />
              </a-form-item>
              <a-form-item :label="t('password')">
                <a-input-password v-model:value="ssh.password" style="width: 200px" />
              </a-form-item>
            </a-form>
          </a-card>

          <a-card :loading="loading" :title="t('mappings')" class="mapping-card">
            <template #extra>
              <a-button type="dashed" @click="addMapping">{{ t("addMapping") }}</a-button>
            </template>
            <div class="mapping-wrap" ref="mappingWrapRef">
              <a-table
                class="mapping-table"
                :columns="columns"
                :data-source="mappings"
                :pagination="false"
                row-key="id"
                size="middle"
                :scroll="{ y: tableScrollY }"
              >
                <template #bodyCell="{ column, record }">
                  <template v-if="column.dataIndex === 'remoteHost'">
                    <a-input
                      v-model:value="record.remoteHost"
                      :placeholder="t('placeholderRemoteHost')"
                      style="width: 100%"
                    />
                  </template>
                  <template v-else-if="column.dataIndex === 'remotePort'">
                    <a-input-number
                      :value="record.remotePort"
                      :min="1"
                      :max="65535"
                      style="width: 100%"
                      @change="(value:any) => onRemotePortChange(record, value)"
                    />
                  </template>
                  <template v-else-if="column.dataIndex === 'localHost'">
                    <a-auto-complete
                      v-model:value="record.localHost"
                      :options="localHostOptions"
                      placeholder="0.0.0.0"
                      style="width: 100%"
                    />
                  </template>
                  <template v-else-if="column.dataIndex === 'localPort'">
                    <a-input-number v-model:value="record.localPort" :min="1" :max="65535" style="width: 100%" />
                  </template>
                  <template v-else-if="column.dataIndex === 'remark'">
                    <a-input v-model:value="record.remark" :placeholder="t('placeholderRemark')" style="width: 100%" />
                  </template>
                  <template v-else-if="column.key === 'actions'">
                    <a-button danger type="link" @click="removeMapping(record.id)">
                      {{ t("delete") }}
                    </a-button>
                  </template>
                </template>
              </a-table>
              <div class="hint">
                {{ t("hint") }}
              </div>
            </div>
          </a-card>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.app {
  min-height: 100vh;
  background: #f7f9fc;
  color: #1c2a3a;
  display: flex;
  flex-direction: column;
  position: relative;
}

.topbar {
  position: sticky;
  top: 0;
  z-index: 10;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 24px;
  background: rgba(255, 255, 255, 0.92);
  backdrop-filter: blur(10px);
  border-bottom: 1px solid rgba(20, 40, 80, 0.1);
}

.brand {
  display: flex;
  align-items: center;
  gap: 12px;
  font-weight: 600;
}

.brand-title {
  font-size: 16px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}

.actions {
  display: flex;
  align-items: center;
  gap: 12px;
}

.lang-select {
  min-width: 96px;
}

.icon-img {
  width: 30px;
  height: 30px;
  margin-top: -4px;
}

.log-panel {
  flex: 1;
  padding: 24px;
  overflow: auto;
  background: #ffffff;
}

.log-list :deep(.ant-list-item) {
  border-bottom: 1px dashed rgba(20, 40, 80, 0.08);
  color: #1c2a3a;
}

.log-list :deep(.ant-space-item) {
  color: inherit;
}

.config-mask {
  position: absolute;
  inset: 0;
  background: rgba(240, 244, 250, 0.9);
  display: flex;
  align-items: stretch;
  justify-content: stretch;
  padding: 0;
  z-index: 20;
}

.config-panel {
  width: 100%;
  height: 100%;
  background: #f5f7fb;
  color: #1b2b4a;
  border-radius: 0;
  display: flex;
  flex-direction: column;
  box-shadow: 0 18px 60px rgba(22, 45, 90, 0.2);
}

.config-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px 20px;
  border-bottom: 1px solid rgba(16, 32, 63, 0.08);
}

.config-title {
  font-weight: 600;
  display: flex;
  gap: 12px;
  align-items: center;
}

.subtle {
  font-size: 12px;
  color: #6c7a95;
}

.config-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.config-body {
  padding: 16px 20px 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  overflow: auto;
  flex: 1;
}

.ssh-form :deep(.ant-form-item) {
  margin-bottom: 12px;
}

.mapping-card {
  flex: 1;
  display: flex;
  flex-direction: column;
}

.mapping-card :deep(.ant-card-body) {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
}

.mapping-wrap {
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex: 1;
  min-height: 0;
}

.mapping-table {
  flex: 1;
  min-height: 0;
}

.hint {
  margin-top: 12px;
  font-size: 12px;
  color: #6b7a99;
}

.empty {
  color: #96a0b5;
}

.time {
  font-variant-numeric: tabular-nums;
  color: #9aa8c2;
}
</style>
