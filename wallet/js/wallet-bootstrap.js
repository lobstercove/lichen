/* Welcome carousel */
(function initWelcomeCarousel() {
    const slides = document.querySelectorAll('.carousel-slide');
    const dots = document.querySelectorAll('.carousel-dot');
    if (!slides.length) return;

    let current = 0;
    let timer = null;
    const intervalMs = 4000;

    function goTo(index) {
        slides[current].classList.remove('active');
        dots[current].classList.remove('active');
        current = (index + slides.length) % slides.length;
        slides[current].classList.add('active');
        dots[current].classList.add('active');
    }

    function startAuto() {
        timer = setInterval(() => {
            goTo(current + 1);
        }, intervalMs);
    }

    function stopAuto() {
        clearInterval(timer);
    }

    dots.forEach((dot) => {
        dot.addEventListener('click', function onDotClick() {
            stopAuto();
            goTo(Number(this.dataset.slide));
            startAuto();
        });
    });

    const track = document.querySelector('.carousel-track');
    if (track) {
        track.addEventListener('mouseenter', stopAuto);
        track.addEventListener('mouseleave', startAuto);
    }

    startAuto();
})();

// Service Worker registration with auto-update
(function registerWalletServiceWorker() {
    if (!('serviceWorker' in navigator)) return;

    navigator.serviceWorker.register('./sw.js').then((registration) => {
        setInterval(() => {
            registration.update();
        }, 30 * 60 * 1000);

        registration.addEventListener('updatefound', () => {
            const newWorker = registration.installing;
            if (!newWorker) return;

            newWorker.addEventListener('statechange', () => {
                if (newWorker.state === 'activated') {
                    window.location.reload();
                }
            });
        });
    }).catch((error) => {
        console.warn('Service worker registration failed:', error);
    });

    navigator.serviceWorker.addEventListener('message', (event) => {
        if (!event.data || event.data.type !== 'SW_UPDATED' || !window.caches) return;

        window.caches.keys().then((keys) => {
            keys.forEach((key) => {
                if (key !== event.data.version) {
                    window.caches.delete(key);
                }
            });
        });
    });
})();