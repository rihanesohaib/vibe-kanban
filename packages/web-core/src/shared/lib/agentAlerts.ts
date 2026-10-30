import type { AgentAlert, SoundFile } from 'shared/types';
import { playSound } from '@/shared/lib/utils';

const CLAIM_PREFIX = 'jarvis.alert.';
const CLAIM_TTL_MS = 60 * 60 * 1000;
const PERMISSION_ASKED_KEY = 'jarvis.notificationPermissionAsked';

function pruneAlertClaims(now: number) {
  for (let i = localStorage.length - 1; i >= 0; i--) {
    const key = localStorage.key(i);
    if (!key?.startsWith(CLAIM_PREFIX)) continue;
    const claimedAt = Number(localStorage.getItem(key));
    if (!Number.isFinite(claimedAt) || now - claimedAt > CLAIM_TTL_MS) {
      localStorage.removeItem(key);
    }
  }
}

function claimInStorage(id: string): boolean {
  const key = CLAIM_PREFIX + id;
  try {
    if (localStorage.getItem(key)) return false;
    const now = Date.now();
    localStorage.setItem(key, String(now));
    pruneAlertClaims(now);
  } catch {
    // Storage unavailable: let this tab alert rather than nobody.
  }
  return true;
}

/**
 * Every open tab receives every alert; only the first tab to claim an alert
 * plays the sound and shows the notification.
 */
export async function claimAlert(id: string): Promise<boolean> {
  if (typeof navigator !== 'undefined' && navigator.locks?.request) {
    try {
      return await navigator.locks.request(CLAIM_PREFIX + id, () =>
        claimInStorage(id)
      );
    } catch {
      // Fall through to the unlocked check.
    }
  }
  return claimInStorage(id);
}

export function browserNotificationsSupported(): boolean {
  return typeof window !== 'undefined' && 'Notification' in window;
}

export function browserNotificationPermission(): NotificationPermission | null {
  return browserNotificationsSupported() ? Notification.permission : null;
}

export async function requestBrowserNotificationPermission(): Promise<NotificationPermission | null> {
  if (!browserNotificationsSupported()) return null;
  if (Notification.permission !== 'default') return Notification.permission;
  try {
    localStorage.setItem(PERMISSION_ASKED_KEY, '1');
  } catch {
    // Ignore storage failures; the prompt itself still works.
  }
  try {
    return await Notification.requestPermission();
  } catch {
    return Notification.permission;
  }
}

/** Whether we already asked once without an explicit user request. */
export function hasAskedNotificationPermission(): boolean {
  try {
    return localStorage.getItem(PERMISSION_ASKED_KEY) === '1';
  } catch {
    return true;
  }
}

export function showBrowserNotification(
  alert: AgentAlert,
  onClick: () => void
): boolean {
  if (browserNotificationPermission() !== 'granted') return false;
  try {
    const notification = new Notification(alert.title, {
      body: alert.body,
      tag: alert.id,
    });
    notification.onclick = () => {
      window.focus();
      onClick();
      notification.close();
    };
    return true;
  } catch (error) {
    console.warn('Failed to show browser notification:', error);
    return false;
  }
}

export function playAlertSound(soundFile: SoundFile): void {
  // Browsers block audio until the user has interacted with the page.
  playSound(`/api/sounds/${soundFile}`).catch((error) => {
    console.warn('Failed to play alert sound:', error);
  });
}
