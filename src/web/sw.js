// MagnetBox service worker — enables installability as a PWA.
// Deliberately minimal: it only caches the two public static assets and lets
// everything else (the authed app, API and media streams) go straight to the
// network, so no private or range responses are ever cached.
const CACHE = 'mb-shell-v1';
const SHELL = ['/icon.svg', '/manifest.webmanifest'];

self.addEventListener('install', (e) => {
  self.skipWaiting();
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).catch(() => {}));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    (async () => {
      const keys = await caches.keys();
      await Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)));
      await self.clients.claim();
    })()
  );
});

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (e.request.method === 'GET' && (url.pathname === '/icon.svg' || url.pathname === '/manifest.webmanifest')) {
    e.respondWith(caches.match(e.request).then((r) => r || fetch(e.request)));
  }
});
