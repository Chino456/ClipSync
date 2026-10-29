const invoke = window.__TAURI__.core.invoke;
const localIp = document.querySelector("#local-ip");
const connectButton = document.querySelector("#connect-button");
const dialog = document.querySelector("#connect-dialog");
const form = document.querySelector("#connect-form");
const peerIp = document.querySelector("#peer-ip");
const dialogError = document.querySelector("#dialog-error");
const cancelButton = document.querySelector("#cancel-button");

let submitting = false;

function openDialog() {
  dialogError.textContent = "";
  dialog.hidden = false;
  requestAnimationFrame(() => peerIp.focus());
}

function closeDialog() {
  dialog.hidden = true;
  dialogError.textContent = "";
}

async function loadDeviceInfo() {
  try {
    const info = await invoke("device_info");
    localIp.textContent = info.localIp;
    connectButton.dataset.deviceName = info.deviceName;
  } catch (error) {
    localIp.textContent = "无法获取本机 IP";
    connectButton.disabled = true;
    console.error(error);
  }
}

async function refreshConnectionStatus() {
  try {
    const status = await invoke("connection_status");
    switch (status.phase) {
      case "connected":
        connectButton.textContent = `已连接：${status.peerName}`;
        connectButton.classList.add("connected");
        break;
      case "connecting":
        connectButton.textContent = status.message ?? "正在连接…";
        connectButton.classList.remove("connected");
        break;
      case "error":
        connectButton.textContent = "连接失败，点击重试";
        connectButton.title = status.message ?? "";
        connectButton.classList.remove("connected");
        break;
      default:
        connectButton.textContent = "开始连接";
        connectButton.classList.remove("connected");
    }
  } catch (error) {
    console.error(error);
  }
}

connectButton.addEventListener("click", openDialog);
cancelButton.addEventListener("click", closeDialog);
dialog.addEventListener("click", (event) => {
  if (event.target === dialog) closeDialog();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !dialog.hidden) closeDialog();
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (submitting) return;

  submitting = true;
  dialogError.textContent = "";
  try {
    await invoke("connect_to_ip", { ip: peerIp.value.trim() });
    closeDialog();
    connectButton.textContent = `正在连接 ${peerIp.value.trim()}…`;
    await refreshConnectionStatus();
  } catch (error) {
    dialogError.textContent = String(error);
    peerIp.focus();
    peerIp.select();
  } finally {
    submitting = false;
  }
});

loadDeviceInfo();
refreshConnectionStatus();
setInterval(refreshConnectionStatus, 500);
