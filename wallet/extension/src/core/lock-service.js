import { patchState } from './state-store.js';

const LOCK_ALARM_NAME = 'lichenwallet-auto-lock';

export async function scheduleAutoLock(lockTimeoutMs) {
  if (!lockTimeoutMs || lockTimeoutMs <= 0) {
    await chrome.alarms.clear(LOCK_ALARM_NAME);
    return;
  }

  await chrome.alarms.create(LOCK_ALARM_NAME, {
    when: Date.now() + lockTimeoutMs
  });
}

export async function clearAutoLockAlarm() {
  await chrome.alarms.clear(LOCK_ALARM_NAME);
}

export async function forceLock() {
  await patchState({ isLocked: true });
}

export function registerLockAlarmHandler() {
  chrome.alarms.onAlarm.addListener(async (alarm) => {
    if (alarm.name !== LOCK_ALARM_NAME) return;
    await forceLock();
  });
}

export { LOCK_ALARM_NAME };
