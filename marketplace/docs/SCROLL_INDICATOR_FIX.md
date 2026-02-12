# Scroll Indicator - Fixed to Match Website ✅

## Problem
Marketplace used a completely different scroll indicator than the website, breaking consistency.

## Website Pattern (Correct) ✅
```css
.scroll-indicator {
    position: absolute;
    bottom: 2rem;
    left: 50%;
    transform: translateX(-50%);
    animation: bounce 2s infinite;
}

@keyframes bounce {
    0%, 20%, 50%, 80%, 100% { transform: translateX(-50%) translateY(0); }
    40% { transform: translateX(-50%) translateY(-10px); }
    60% { transform: translateX(-50%) translateY(-5px); }
}

.scroll-arrow {
    width: 30px;
    height: 30px;
    border-left: 2px solid var(--primary);
    border-bottom: 2px solid var(--primary);
    transform: rotate(-45deg);
}
```

**Design**: Simple arrow made with borders, rotated 45 degrees, bouncing animation

## Marketplace (Before) ❌
```css
.scroll-indicator {
    position: absolute;
    bottom: 2rem;
    left: 50%;
    transform: translateX(-50%);
}

.scroll-arrow {
    width: 30px;
    height: 50px;
    border: 2px solid var(--primary);
    border-radius: 25px;
    position: relative;
}

.scroll-arrow::before {
    content: '';
    position: absolute;
    top: 10px;
    left: 50%;
    transform: translateX(-50%);
    width: 6px;
    height: 6px;
    background: var(--primary);
    border-radius: 50%;
    animation: scrollDown 2s infinite;
}
```

**Design**: Rounded rectangle with dot inside - COMPLETELY DIFFERENT!

## Fix Applied ✅

Replaced marketplace scroll indicator with **exact copy** from website:
- Same arrow shape (30x30px with rotated borders)
- Same bounce animation
- Same positioning
- Same styling

## Result

**Now 100% consistent** with website scroll indicator.

---

**Trading Lobster** 🦞⚡  
*Every detail must match. No exceptions.*
