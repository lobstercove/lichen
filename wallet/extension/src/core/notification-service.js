export async function notify(title, message) {
  try {
    await chrome.runtime.sendMessage({
      type: 'LICHEN_NOTIFY',
      payload: { title, message }
    });
  } catch {
    // no-op
  }
}
