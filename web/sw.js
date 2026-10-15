// Number Loom's service worker, so that the app (and the installed PWA) opens offline.
//
// Network first, with the cache as a fallback: GitHub Pages redeploys on every push to `main`, and
// a cache-first worker would go on serving the old build. Only same-origin GETs are touched.

const CACHE = 'number-loom';

self.addEventListener('install', (event) => {
    // The page that registered this worker loaded before there was a worker to see it, so cache
    // what it's made of now. Otherwise going offline right after installing would find nothing.
    event.waitUntil(precacheShell());
    self.skipWaiting();
});

self.addEventListener('activate', (event) => {
    event.waitUntil((async () => {
        for (const name of await caches.keys()) {
            if (name !== CACHE) {
                await caches.delete(name);
            }
        }
        await self.clients.claim();
    })());
});

self.addEventListener('fetch', (event) => {
    const request = event.request;
    if (request.method !== 'GET' || new URL(request.url).origin !== self.location.origin) {
        return;
    }
    event.respondWith(networkFirst(event));
});

async function networkFirst(event) {
    const request = event.request;
    const navigating = request.mode === 'navigate';
    try {
        const response = await fetch(request);
        if (response.status === 200) {
            event.waitUntil(remember(navigating ? withoutSearch(request.url) : request.url, response.clone()));
        }
        return response;
    } catch (error) {
        // Offline, or the server can't be reached. Any URL for the app itself gets the app,
        // whatever its query string.
        const cached = await caches.match(request, { ignoreSearch: navigating });
        if (cached) {
            return cached;
        }
        throw error;
    }
}

async function precacheShell() {
    const cache = await caches.open(CACHE);
    const page = await fetch('./', { cache: 'no-cache' });
    if (page.status !== 200) {
        return;
    }
    const html = await page.clone().text();
    await cache.put(new URL('./', self.location).href, page);
    // Trunk links the JS glue, the wasm, and any JS snippets from the page's <head>, as `./...`.
    const assets = [...html.matchAll(/(?:href|src)="(\.\/[^"]+)"/g)].map((match) => match[1]);
    await Promise.all(assets.map(async (asset) => {
        try {
            await cache.add(asset);
        } catch (error) {
            // Not worth failing the install over; the next online load will cache it.
        }
    }));
}

async function remember(url, response) {
    const cache = await caches.open(CACHE);
    // Trunk names the wasm, its JS glue, and the snippet directories with a hash of the build, so
    // each new build's copies arrive under new names. Forget the old build's copies as the new
    // ones arrive, or they'd pile up forever.
    const slot = buildSlot(url);
    for (const key of await cache.keys()) {
        if (key.url !== url && buildSlot(key.url) === slot) {
            await cache.delete(key);
        }
    }
    await cache.put(url, response);
}

function buildSlot(url) {
    return new URL(url).pathname.replace(/-[0-9a-f]{16}(?=[._/])/g, '');
}

function withoutSearch(url) {
    const stripped = new URL(url);
    stripped.search = '';
    stripped.hash = '';
    return stripped.href;
}
