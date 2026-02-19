import { loadState } from '../core/state-store.js';
import { loadNftDetails } from '../core/nft-service.js';

const MARKETPLACE_URL = 'https://moltchain.network/nft-marketplace';

function escapeHtml(str) {
  if (!str) return '';
  return String(str).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#x27;');
}

function safeImageUrl(url) {
  if (!url) return '';
  try {
    const parsed = new URL(String(url));
    if (parsed.protocol === 'https:' || parsed.protocol === 'http:' || parsed.protocol === 'ipfs:') return parsed.href;
    return '';
  } catch { return ''; }
}

function setHtml(id, html) {
  const node = document.getElementById(id);
  if (node) node.innerHTML = html;
}

function shortAddress(address) {
  if (!address) return '—';
  return address.length > 18 ? `${address.slice(0, 10)}...${address.slice(-8)}` : address;
}

function setStatus(message) {
  const node = document.getElementById('nftsStatus');
  if (node) node.textContent = message;
}

function copyText(value) {
  if (!value) return Promise.reject(new Error('Nothing to copy'));
  return navigator.clipboard.writeText(value);
}

async function loadNftPage() {
  const state = await loadState();
  const wallet = state.wallets?.find((w) => w.id === state.activeWalletId) || null;
  if (!wallet) {
    setHtml('nftsMeta', 'No active wallet. Open popup to create/import wallet.');
    setHtml('nftsGrid', '');
    setStatus('No active wallet');
    return;
  }

  const network = state.network?.selected || 'local-testnet';
  setHtml('nftsMeta', `<strong>${wallet.name}</strong> • ${network} • ${shortAddress(wallet.address)}`);

  setStatus('Loading NFTs...');
  const items = await loadNftDetails(wallet.address, network).catch(() => []);
  if (!items.length) {
    setHtml('nftsGrid', '<div class="nft-card"><div class="nft-body">No NFTs found for this wallet.</div></div>');
    setStatus('No NFTs found');
    return;
  }

  setStatus(`Loaded ${items.length} NFT${items.length === 1 ? '' : 's'}`);

  setHtml('nftsGrid', items.map((nft) => {
    const safeSrc = safeImageUrl(nft.image);
    const safeName = escapeHtml(nft.name);
    const safeMint = escapeHtml(nft.mint);
    const safeStandard = escapeHtml(nft.standard);
    const safeSymbol = escapeHtml(nft.symbol);
    const safeAmount = escapeHtml(String(nft.amount));
    return `
    <article class="nft-card" data-mint="${safeMint}">
      <div class="nft-image">${safeSrc ? `<img src="${safeSrc}" alt="${safeName}" style="max-width:100%;max-height:100%;object-fit:cover;" />` : 'No Image'}</div>
      <div class="nft-body">
        <div class="nft-title">${safeName}</div>
        <div><strong>Standard:</strong> ${safeStandard}</div>
        <div><strong>Symbol:</strong> ${safeSymbol}</div>
        <div><strong>Amount:</strong> ${safeAmount}</div>
        <div><strong>Mint:</strong> ${safeMint}</div>
        <div class="nft-card-actions">
          <button class="btn btn-secondary btn-small" data-action="copyMint" data-mint="${safeMint}">Copy Mint</button>
          <button class="btn btn-secondary btn-small" data-action="openMarket">View Market</button>
        </div>
      </div>
    </article>
  `;
  }).join(''));
}

function handleGridClick(event) {
  const target = event.target;
  if (!(target instanceof HTMLElement)) return;

  const action = target.dataset?.action;
  if (!action) return;

  if (action === 'copyMint') {
    const mint = target.dataset?.mint || '';
    copyText(mint)
      .then(() => setStatus(`Mint copied: ${mint.slice(0, 10)}...`))
      .catch((error) => setStatus(`Copy failed: ${error?.message || error}`));
    return;
  }

  if (action === 'openMarket') {
    chrome.tabs.create({ url: MARKETPLACE_URL });
    setStatus('Opened marketplace in a new tab');
  }
}

document.getElementById('refreshNfts')?.addEventListener('click', loadNftPage);
document.getElementById('openMarketplace')?.addEventListener('click', () => {
  chrome.tabs.create({ url: MARKETPLACE_URL });
  setStatus('Opened marketplace in a new tab');
});
document.getElementById('nftsGrid')?.addEventListener('click', handleGridClick);
document.getElementById('backHome')?.addEventListener('click', () => {
  location.href = chrome.runtime.getURL('src/pages/home.html');
});

loadNftPage();
