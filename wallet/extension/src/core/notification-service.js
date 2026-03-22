export async function notify(title, message) {
  try {
    await chrome.runtime.sendMessage({
      type: 'MOLT_NOTIFY',
      payload: { title, message }
    });
  } catch {
    // no-op
  }
}
