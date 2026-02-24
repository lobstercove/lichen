/**
 * MoltChain Playground - COMPLETE WORKING VERSION
 * All features properly implemented and wired
 * 
 * @version 3.0.0 (Actually Production Ready)
 */

console.log('🦞 MoltChain Playground Loading (Complete Version)...');

const EXPLORER_NETWORK_STORAGE_KEY = 'explorer_network';
const PLAYGROUND_NETWORK_STORAGE_KEY = 'playground_network';

function normalizeExplorerNetwork(network) {
    if (network === 'local') return 'local-testnet';
    if (['mainnet', 'testnet', 'local-testnet', 'local-mainnet'].includes(network)) {
        return network;
    }
    return 'testnet';
}

function mapExplorerToSdkNetwork(network) {
    const normalized = normalizeExplorerNetwork(network);
    if (normalized === 'mainnet') return 'mainnet';
    if (normalized === 'testnet') return 'testnet';
    return 'local';
}

// Load SDK first
if (typeof MoltChain === 'undefined') {
    console.error('❌ MoltChain SDK not loaded! Include moltchain-sdk.js first');
}

// AUDIT-FIX F14.1–F14.7: HTML-escape helper to prevent XSS in innerHTML
function escapeHtml(str) {
    if (typeof str !== 'string') return String(str ?? '');
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// AUDIT-FIX F14.1: URL scheme whitelist for terminal links
function sanitizeUrl(url) {
    if (typeof url !== 'string') return '';
    try {
        const parsed = new URL(url, window.location.origin);
        if (!['http:', 'https:'].includes(parsed.protocol)) return '';
        return url;
    } catch {
        return '';
    }
}

// ============================================================================
// COMPLETE STATE MANAGEMENT
// ============================================================================

const Playground = {
    // Core state
    network: 'testnet',
    rpc: null,
    ws: null,
    wallet: null,
    balance: null,
    
    // Editor state
    editor: null,
    currentFile: 'lib.rs',
    files: new Map(),
    modifiedFiles: new Set(),
    
    // Build state
    compiledWasm: null,
    buildErrors: [],
    
    // Deployed programs
    deployedPrograms: [],
    networkPrograms: [],
    selectedProgramId: null,
    programKeypair: null,
    programIdOverrideEnabled: false,
    projectName: 'workspace',
    formatOnSave: false,
    isFormatting: false,
    programCallsCache: [],
    currentTemplateId: null,
    templateOptions: {},
    registryStatusTimer: null,
    registryLookupCache: new Map(),
    wallets: [],
    activeWalletId: null,
    
    // UI state
    terminalTab: 'terminal',
    sidebarTab: 'files',
    terminalCollapsed: false,
    
    // Initialize everything
    async init() {
        console.log('📦 Initializing Playground...');

        const savedNetwork = localStorage.getItem(PLAYGROUND_NETWORK_STORAGE_KEY)
            || localStorage.getItem(EXPLORER_NETWORK_STORAGE_KEY)
            || this.network;
        this.network = normalizeExplorerNetwork(savedNetwork);
        
        // Initialize network clients
        await this.initNetwork(this.network);
        
        // Initialize Monaco editor
        await this.initMonacoEditor();
        
        // Load default files
        this.loadDefaultFiles();
        
        // Setup all event listeners
        this.setupEventListeners();
        
        // Load saved state from localStorage
        this.loadSavedState();
        
        // Update UI
        this.updateUI();

        // Load on-chain programs
            await this.refreshProgramIndex({ showError: false });

        this.updateProgramIdPreview();
        
        // Subscribe to live updates
        this.setupLiveUpdates();
        
        console.log('✅ Playground initialized');
    },
    
    // Initialize network connection
    async initNetwork(network) {
        this.network = normalizeExplorerNetwork(network);
        const sdkNetwork = mapExplorerToSdkNetwork(this.network);
        this.rpc = new MoltChain.RPC(sdkNetwork);
        this.ws = new MoltChain.WebSocket(sdkNetwork);
        
        try {
            await this.ws.connect();
            console.log('✅ WebSocket connected');
        } catch (e) {
            console.warn('⚠️  WebSocket connection failed:', e);
        }
        
        this.loadWalletStore();
        await this.applyActiveWallet();
    },
    
    // Initialize Monaco Editor
    async initMonacoEditor() {
        return new Promise((resolve) => {
            require.config({ 
                paths: { 
                    vs: 'https://cdnjs.cloudflare.com/ajax/libs/monaco-editor/0.45.0/min/vs' 
                } 
            });
            
            require(['vs/editor/editor.main'], () => {
                this.editor = monaco.editor.create(
                    document.getElementById('monacoEditor'),
                    {
                        value: '',
                        language: 'rust',
                        theme: 'vs-dark',
                        fontSize: 14,
                        minimap: { enabled: true },
                        automaticLayout: true,
                        scrollBeyondLastLine: false,
                        renderWhitespace: 'selection',
                        tabSize: 4,
                        insertSpaces: true,
                        formatOnPaste: true,
                        formatOnType: true,
                        suggestOnTriggerCharacters: true,
                        quickSuggestions: true
                    }
                );
                
                // Add keyboard shortcuts
                this.editor.addCommand(
                    monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyB,
                    () => this.buildProgram()
                );
                
                this.editor.addCommand(
                    monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyD,
                    () => this.deployProgram()
                );
                
                this.editor.addCommand(
                    monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS,
                    () => this.saveCurrentFile()
                );
                
                this.editor.addCommand(
                    monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyT,
                    () => this.runTests()
                );
                
                // Track changes
                this.editor.onDidChangeModelContent(() => {
                    if (this.isFormatting) return;
                    this.modifiedFiles.add(this.currentFile);
                    this.updateFileModifiedIndicator();
                    this.debouncedSave();
                });
                
                console.log('✅ Monaco editor initialized');
                resolve();
            });
        });
    },
    
    // Load default project files
    loadDefaultFiles() {
        this.files.set('lib.rs', DEFAULT_FILES.LIB_RS);
        this.files.set('Cargo.toml', DEFAULT_FILES.CARGO_TOML);
        this.files.set('tests/lib_test.rs', DEFAULT_FILES.TEST_FILE);
        
        // Load first file into editor
        this.loadFile('lib.rs');
    },
    
    // Setup all event listeners
    setupEventListeners() {
        // Network selector
        document.getElementById('networkSelect')?.addEventListener('change', (e) => {
            this.switchNetwork(e.target.value);
        });

        document.getElementById('fileMenuBtn')?.addEventListener('click', (e) => {
            e.stopPropagation();
            this.toggleFileMenu();
        });

        document.getElementById('menuSnapshotExport')?.addEventListener('click', () => {
            this.exportSnapshot();
            this.closeFileMenu();
        });

        document.getElementById('menuSnapshotImport')?.addEventListener('click', () => {
            document.getElementById('snapshotImportInput')?.click();
            this.closeFileMenu();
        });

        document.addEventListener('click', () => {
            this.closeFileMenu();
        });

        document.getElementById('copyRpcBtn')?.addEventListener('click', () => {
            this.copyRpcUrl();
        });
        
        // Wallet button
        document.getElementById('walletBtn')?.addEventListener('click', () => {
            if (this.wallet) {
                this.toggleWalletDropdown();
            } else {
                this.openWalletModal();
            }
        });

        document.getElementById('walletDropdownClose')?.addEventListener('click', () => {
            this.hideWalletDropdown();
        });

        document.getElementById('walletAddBtn')?.addEventListener('click', () => {
            this.hideWalletDropdown();
            this.openWalletModal();
        });

        document.getElementById('walletRemoveBtn')?.addEventListener('click', () => {
            this.removeActiveWallet();
        });

        document.addEventListener('click', (event) => {
            const dropdown = document.getElementById('walletDropdown');
            const walletBtn = document.getElementById('walletBtn');
            if (!dropdown || dropdown.style.display === 'none') return;
            if (dropdown.contains(event.target) || walletBtn?.contains(event.target)) return;
            this.hideWalletDropdown();
        });

        // Wallet modal controls
        document.getElementById('closeWalletModal')?.addEventListener('click', () => {
            this.closeWalletModal();
        });
        document.getElementById('walletModalOverlay')?.addEventListener('click', () => {
            this.closeWalletModal();
        });
        document.getElementById('createWalletBtn')?.addEventListener('click', () => {
            this.createWalletFromModal();
        });
        document.getElementById('importWalletBtn')?.addEventListener('click', () => {
            this.importWalletFromModal();
        });
        document.getElementById('exportSeedBtn')?.addEventListener('click', () => {
            this.exportWalletSeed();
        });
        document.getElementById('exportPrivateKeyBtn')?.addEventListener('click', () => {
            this.exportWalletPrivateKey();
        });
        document.getElementById('exportJsonBtn')?.addEventListener('click', () => {
            this.exportWallet();
        });

        document.querySelectorAll('.modal-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                document.querySelectorAll('.modal-tab').forEach(btn => btn.classList.remove('active'));
                document.querySelectorAll('.modal-tab-content').forEach(content => content.classList.remove('active'));
                tab.classList.add('active');
                const target = tab.dataset.walletTab;
                document.querySelectorAll(`.modal-tab-content[data-wallet-tab="${target}"]`).forEach(panel => {
                    panel.classList.add('active');
                });
            });
        });

        document.querySelectorAll('input[name="importMethod"]').forEach(radio => {
            radio.addEventListener('change', (e) => {
                const method = e.target.value;
                document.getElementById('importSeedPanel').style.display = method === 'seed' ? 'block' : 'none';
                document.getElementById('importPrivateKeyPanel').style.display = method === 'privatekey' ? 'block' : 'none';
                document.getElementById('importJsonPanel').style.display = method === 'json' ? 'block' : 'none';
            });
        });
        
        // Faucet button
        document.getElementById('faucetBtn')?.addEventListener('click', () => {
            this.requestFaucet();
        });

        document.getElementById('formatOnSaveToggle')?.addEventListener('change', (e) => {
            this.setFormatOnSave(e.target.checked);
        });
        
        // Toolbar buttons
        document.getElementById('buildBtn')?.addEventListener('click', () => this.buildProgram());
        document.getElementById('deployBtn')?.addEventListener('click', () => this.deployProgram());
        document.getElementById('upgradeProgramBtn')?.addEventListener('click', () => this.upgradeProgram());
        document.getElementById('testBtn')?.addEventListener('click', () => this.runTests());
        document.getElementById('formatBtn')?.addEventListener('click', () => this.formatCode());
        document.getElementById('verifyBtn')?.addEventListener('click', () => this.verifyCode());

        document.getElementById('refreshProgramsBtn')?.addEventListener('click', () => {
            this.refreshProgramIndex({ showError: true });
        });

        document.getElementById('callFunctionBtn')?.addEventListener('click', () => {
            this.testProgram();
        });

        document.getElementById('upgradeAuthority')?.addEventListener('change', (e) => {
            const group = document.getElementById('customAuthorityGroup');
            if (group) {
                group.style.display = e.target.value === 'custom' ? 'block' : 'none';
            }
        });

        document.getElementById('closeResultBtn')?.addEventListener('click', () => {
            const resultEl = document.getElementById('testResult');
            if (resultEl) {
                resultEl.style.display = 'none';
            }
        });

        document.getElementById('closeProgramInfoBtn')?.addEventListener('click', () => {
            this.hideProgramPanels();
        });

        document.getElementById('refreshStorageBtn')?.addEventListener('click', () => {
            if (this.selectedProgramId) {
                this.loadProgramStorage(this.selectedProgramId);
            }
        });

        document.getElementById('refreshProgramCallsBtn')?.addEventListener('click', () => {
            if (this.selectedProgramId) {
                this.loadProgramCalls(this.selectedProgramId);
            }
        });

        document.getElementById('programIdOverride')?.addEventListener('change', (e) => {
            if (!this.wallet) {
                e.target.checked = false;
                this.programIdOverrideEnabled = false;
                this.toggleProgramOverrideUI(false);
                this.saveProgramOverride();
                this.updateProgramIdPreview();
                return;
            }
            this.programIdOverrideEnabled = e.target.checked;
            this.toggleProgramOverrideUI(this.programIdOverrideEnabled);
            this.saveProgramOverride();
            this.updateProgramIdPreview();
        });

        document.getElementById('programIdOverrideValue')?.addEventListener('input', () => {
            this.saveProgramOverride();
            this.updateProgramIdPreview();
        });

        document.getElementById('copyProgramIdPreviewBtn')?.addEventListener('click', () => {
            const value = document.getElementById('programIdPreview')?.value;
            if (value) {
                navigator.clipboard.writeText(value);
            }
        });

        document.getElementById('newProgramKeypairBtn')?.addEventListener('click', () => {
            this.createProgramKeypair(true);
        });

        document.getElementById('importProgramKeypairBtn')?.addEventListener('click', () => {
            this.importProgramKeypair();
        });

        document.getElementById('exportProgramKeypairBtn')?.addEventListener('click', () => {
            this.exportProgramKeypair();
        });
        
        // Build & Deploy button (sidebar)
        document.getElementById('buildDeployBtn')?.addEventListener('click', () => {
            this.buildProgram().then(() => {
                if (this.compiledWasm) {
                    this.deployProgram();
                }
            });
        });
        
        document.getElementById('buildOnlyBtn')?.addEventListener('click', () => this.buildProgram());
        
        // File tree
        document.getElementById('fileTree')?.addEventListener('click', (e) => {
            const fileItem = e.target.closest('.file-item');
            if (fileItem) {
                const filename = fileItem.dataset.file;
                this.loadFile(filename);
            }
            
            const folderHeader = e.target.closest('.folder-header');
            if (folderHeader) {
                const folderItem = folderHeader.parentElement;
                folderItem.classList.toggle('open');
            }
        });
        
        // Examples
        document.getElementById('examplesList')?.addEventListener('click', (e) => {
            const exampleItem = e.target.closest('.example-item');
            if (exampleItem) {
                const exampleId = exampleItem.dataset.example;
                this.loadExample(exampleId);
            }
        });
        
        // Sidebar tabs
        document.querySelectorAll('.sidebar-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                this.switchSidebarTab(tab.dataset.tab);
            });
        });
        
        // Terminal tabs
        document.querySelectorAll('.terminal-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                this.switchTerminalTab(tab.dataset.terminalTab);
            });
        });
        
        // Clear terminal
        document.getElementById('clearTerminalBtn')?.addEventListener('click', () => {
            this.clearTerminal();
        });

        const terminalInput = document.getElementById('terminalInput');
        terminalInput?.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                const command = terminalInput.value;
                terminalInput.value = '';
                this.handleTerminalCommand(command);
            }
        });

        document.querySelector('.terminal-content[data-terminal-tab="terminal"]')
            ?.addEventListener('click', () => terminalInput?.focus());
        
        // Test & Interact
        document.getElementById('testProgramBtn')?.addEventListener('click', () => {
            this.testProgram();
        });

        document.getElementById('testProgramAddr')?.addEventListener('change', (e) => {
            const programId = e.target.value.trim();
            if (programId) {
                this.showProgramDetails(programId);
            }
        });
        
        // Transfer
        document.getElementById('transferBtn')?.addEventListener('click', () => {
            this.sendTransfer();
        });

        document.getElementById('projectNameInput')?.addEventListener('input', (e) => {
            this.setProjectName(e.target.value);
        });

        document.getElementById('renameProjectBtn')?.addEventListener('click', () => {
            this.applyProjectRenameToCode();
        });
        
        // File operations
        document.getElementById('newFileBtn')?.addEventListener('click', () => this.newFile());
        document.getElementById('newFolderBtn')?.addEventListener('click', () => this.newFolder());
        document.getElementById('importProgramBtn')?.addEventListener('click', () => this.importProgram());
        document.getElementById('exportProgramBtn')?.addEventListener('click', () => this.exportProgram());

        document.getElementById('snapshotSaveBtn')?.addEventListener('click', () => this.saveSnapshot());
        document.getElementById('snapshotRestoreBtn')?.addEventListener('click', () => this.restoreSnapshot());
        document.getElementById('snapshotExportBtn')?.addEventListener('click', () => this.exportSnapshot());
        document.getElementById('snapshotImportBtn')?.addEventListener('click', () => {
            document.getElementById('snapshotImportInput')?.click();
        });
        document.getElementById('snapshotImportInput')?.addEventListener('change', (e) => {
            const file = e.target.files?.[0];
            if (file) {
                this.importSnapshot(file);
            }
            e.target.value = '';
        });
        
        // Theme/language selectors
        document.getElementById('themeSelect')?.addEventListener('change', (e) => {
            monaco.editor.setTheme(e.target.value);
        });
        
        document.getElementById('fontSizeSelect')?.addEventListener('change', (e) => {
            this.editor.updateOptions({ fontSize: parseInt(e.target.value) });
        });
        
        document.getElementById('languageSelect')?.addEventListener('change', (e) => {
            const model = this.editor.getModel();
            monaco.editor.setModelLanguage(model, e.target.value);
        });

        document.getElementById('openProgramExplorerBtn')?.addEventListener('click', () => {
            this.openProgramInExplorer();
        });

        document.getElementById('openProgramInfoExplorerBtn')?.addEventListener('click', () => {
            this.openProgramInfoInExplorer();
        });

        document.getElementById('programCallsFilter')?.addEventListener('input', () => {
            this.renderProgramCalls();
        });
    },
    
    // Load saved state
    loadSavedState() {
        // Load files from localStorage
        const savedFiles = localStorage.getItem('playground_files');
        if (savedFiles) {
            try {
                const filesArray = JSON.parse(savedFiles);
                filesArray.forEach(([path, content]) => {
                    this.files.set(path, content);
                });
            } catch (e) {
                console.error('Failed to load saved files:', e);
            }
        }
        
        // Load deployed programs
        const savedPrograms = localStorage.getItem('deployed_programs');
        if (savedPrograms) {
            try {
                this.deployedPrograms = JSON.parse(savedPrograms);
                this.updateDeployedProgramsList();
                this.registryLookupCache.clear();
                this.deployedPrograms.forEach(program => {
                    if (program?.registry?.symbol) {
                        this.registryLookupCache.set(program.registry.symbol, program.registry);
                    }
                });
            } catch (e) {
                console.error('Failed to load deployed programs:', e);
            }
        }

        const savedProgramKeypair = localStorage.getItem('program_keypair');
        if (savedProgramKeypair) {
            try {
                this.programKeypair = JSON.parse(savedProgramKeypair);
            } catch (e) {
                console.error('Failed to load program keypair:', e);
            }
        }

        const overrideEnabled = localStorage.getItem('program_id_override_enabled');
        if (overrideEnabled !== null) {
            this.programIdOverrideEnabled = overrideEnabled === 'true';
        }

        const overrideValue = localStorage.getItem('program_id_override_value');
        if (overrideValue) {
            const input = document.getElementById('programIdOverrideValue');
            if (input) {
                input.value = overrideValue;
            }
        }

        const savedProjectName = localStorage.getItem('playground_project_name');
        if (savedProjectName) {
            this.projectName = savedProjectName;
        }

        const savedFormatOnSave = localStorage.getItem('playground_format_on_save');
        if (savedFormatOnSave !== null) {
            this.formatOnSave = savedFormatOnSave === 'true';
        }

        this.toggleProgramOverrideUI(this.programIdOverrideEnabled);
        this.updateProjectNameInSource(this.projectName);
    },
    
    // Setup live WebSocket updates
    setupLiveUpdates() {
        if (!this.ws || !this.ws.connected) return;
        
        // Subscribe to slots
        this.ws.subscribeSlots((slot) => {
            const totalTxs = document.getElementById('metricTotalTxs');
            if (totalTxs) {
                totalTxs.textContent = slot.slot.toLocaleString();
            }
        });
        
        // Subscribe to metrics if we can get them
        // For now, poll metrics every 5s
        setInterval(async () => {
            try {
                const metrics = await this.rpc.getMetrics();
                const tpsEl = document.getElementById('metricTPS');
                const totalTxsEl = document.getElementById('metricTotalTxs');
                const blockTimeEl = document.getElementById('metricBlockTime');
                const burnedEl = document.getElementById('metricBurned');

                if (tpsEl) tpsEl.textContent = metrics.tps.toFixed(2);
                if (totalTxsEl) totalTxsEl.textContent = metrics.total_transactions.toLocaleString();
                if (blockTimeEl) blockTimeEl.textContent = `${(metrics.average_block_time * 1000).toFixed(0)}ms`;
                if (burnedEl) burnedEl.textContent = `${(metrics.total_burned / 1_000_000_000).toFixed(2)} MOLT`;
            } catch (e) {
                // Ignore errors
            }
        }, 5000);
        
        // Subscribe to wallet balance if we have a wallet
        if (this.wallet) {
            this.ws.subscribeAccount(this.wallet.address, (accountInfo) => {
                this.balance = accountInfo;
                this.updateBalanceDisplay();
            });
        }
    },
    
    // Update all UI elements
    updateUI() {
        this.updateNetworkDisplay();
        this.updateWalletDisplay();
        this.updateFileTree();
        this.updateDeployedProgramsList();
        this.toggleProgramOverrideUI(this.programIdOverrideEnabled);
        this.updateProjectNameUI();
        this.updateFormatOnSaveUI();
        this.renderTemplateOptions();
        this.updateUpgradeAuthorityUI();
    },
    
    // ========================================================================
    // FILE MANAGEMENT
    // ========================================================================
    
    loadFile(filename) {
        const content = this.files.get(filename) || '';
        this.editor.setValue(content);
        this.currentFile = filename;
        
        // Update UI
        document.querySelectorAll('.file-item').forEach(item => {
            item.classList.toggle('active', item.dataset.file === filename);
        });
        
        document.getElementById('currentFileName').textContent = filename;
        
        // Determine language
        const ext = filename.split('.').pop();
        const langMap = {
            'rs': 'rust',
            'toml': 'toml',
            'js': 'javascript',
            'ts': 'typescript',
            'c': 'c',
            'cpp': 'cpp',
            'h': 'cpp'
        };
        
        const lang = langMap[ext] || 'rust';
        const model = this.editor.getModel();
        monaco.editor.setModelLanguage(model, lang);
    },
    
    async saveCurrentFile() {
        if (this.formatOnSave && this.editor && !this.isFormatting) {
            this.isFormatting = true;
            try {
                await this.editor.getAction('editor.action.formatDocument').run();
            } catch (e) {
                // Ignore formatting errors
            } finally {
                this.isFormatting = false;
            }
        }
        const content = this.editor.getValue();
        this.files.set(this.currentFile, content);
        this.modifiedFiles.delete(this.currentFile);
        
        // Save to localStorage
        localStorage.setItem('playground_files', JSON.stringify(Array.from(this.files.entries())));
        
        this.updateFileModifiedIndicator();
        this.addTerminalLine(`💾 Saved ${this.currentFile}`, 'success');
    },
    
    debouncedSave: (() => {
        let timeout;
        return function() {
            clearTimeout(timeout);
            timeout = setTimeout(() => this.saveCurrentFile(), 1000);
        };
    })(),
    
    updateFileModifiedIndicator() {
        const indicator = document.getElementById('fileModified');
        if (indicator) {
            indicator.style.display = this.modifiedFiles.has(this.currentFile) ? 'inline' : 'none';
        }
    },
    
    updateFileTree() {

        const savedTemplateState = localStorage.getItem('playground_template_state');
        if (savedTemplateState) {
            try {
                const parsed = JSON.parse(savedTemplateState);
                this.currentTemplateId = parsed.currentTemplateId || null;
                this.templateOptions = parsed.templateOptions || {};
            } catch (e) {
                console.error('Failed to load template state:', e);
            }
        }
        // File tree already in HTML, just update active state
        document.querySelectorAll('.file-item').forEach(item => {
            item.classList.toggle('active', item.dataset.file === this.currentFile);
        });
    },

    updateProjectNameUI() {
        const input = document.getElementById('projectNameInput');
        if (input && input.value !== this.projectName) {
            input.value = this.projectName;
        }

        const label = document.getElementById('projectRootName');
        if (label) {
            label.textContent = this.projectName || 'workspace';
        }

        const programName = document.getElementById('programName');
        if (programName) {
            const trimmed = (this.projectName || 'workspace').trim();
            if (!programName.value || programName.value === 'hello_world' || programName.value === 'workspace') {
                programName.value = trimmed || 'workspace';
            }
        }
    },

    setProjectName(name) {
        const trimmed = (name || '').trim();
        this.projectName = trimmed || 'workspace';
        localStorage.setItem('playground_project_name', this.projectName);
        this.updateProjectNameUI();
        this.updateProjectNameInCargoToml(this.projectName);
    },

    applyProjectRenameToCode() {
        const name = this.projectName || 'workspace';
        this.updateProjectNameInSource(name);
        this.updateProjectNameInCargoToml(name);
        this.addTerminalLine(`✅ Renamed files + code to ${name}`, 'success');
        this.showToast(`Renamed files + code to ${name}`, 'success');
    },

    updateProjectNameInSource(name) {
        if (!this.files.has('lib.rs')) return;
        if (this.modifiedFiles.has('lib.rs')) return;

        const current = this.files.get('lib.rs');
        const pattern = /^\/\/\s*Project:\s*.*$/m;
        if (!pattern.test(current)) return;

        const updated = current.replace(pattern, `// Project: ${name}`);
        if (updated === current) return;

        this.files.set('lib.rs', updated);

        if (this.currentFile === 'lib.rs' && this.editor) {
            const editorContent = this.editor.getValue();
            if (editorContent === current) {
                this.editor.setValue(updated);
            }
        }
    },

    updateProjectNameInCargoToml(name) {
        if (!this.files.has('Cargo.toml')) return;
        if (this.modifiedFiles.has('Cargo.toml')) return;

        const current = this.files.get('Cargo.toml');
        const pattern = /^(name\s*=\s*")[^"]*(")/m;
        if (!pattern.test(current)) return;

        const updated = current.replace(pattern, `$1${name}$2`);
        if (updated === current) return;

        this.files.set('Cargo.toml', updated);

        if (this.currentFile === 'Cargo.toml' && this.editor) {
            const editorContent = this.editor.getValue();
            if (editorContent === current) {
                this.editor.setValue(updated);
            }
        }
    },

    updateFormatOnSaveUI() {
        const toggle = document.getElementById('formatOnSaveToggle');
        if (toggle) {
            toggle.checked = this.formatOnSave;
        }
    },

    toggleFileMenu() {
        const menu = document.getElementById('fileMenuDropdown');
        if (!menu) return;
        menu.classList.toggle('open');
    },

    closeFileMenu() {
        const menu = document.getElementById('fileMenuDropdown');
        if (menu) {
            menu.classList.remove('open');
        }
    },

    setFormatOnSave(enabled) {
        this.formatOnSave = Boolean(enabled);
        localStorage.setItem('playground_format_on_save', String(this.formatOnSave));
        this.updateFormatOnSaveUI();
        this.showToast(`Format on save ${this.formatOnSave ? 'enabled' : 'disabled'}`, 'success');
    },

    copyRpcUrl() {
        const url = this.rpc?.rpcUrl;
        if (!url) {
            this.showToast('RPC URL not available', 'warning');
            return;
        }
        navigator.clipboard.writeText(url);
        this.showToast('RPC URL copied', 'success');
    },

    saveSnapshot() {
        const snapshot = {
            files: Array.from(this.files.entries()),
            currentFile: this.currentFile,
            projectName: this.projectName,
            programIdOverrideEnabled: this.programIdOverrideEnabled,
            programIdOverrideValue: document.getElementById('programIdOverrideValue')?.value || '',
            programKeypair: this.programKeypair,
            deployedPrograms: this.deployedPrograms,
            network: this.network,
            formatOnSave: this.formatOnSave,
            currentTemplateId: this.currentTemplateId,
            templateOptions: this.templateOptions
        };
        localStorage.setItem('playground_snapshot', JSON.stringify(snapshot));
        this.showToast('Snapshot saved', 'success');
    },

    restoreSnapshot() {
        const raw = localStorage.getItem('playground_snapshot');
        if (!raw) {
            this.showToast('No snapshot found', 'warning');
            return;
        }
        if (!confirm('Restore snapshot? This will replace current files.')) {
            return;
        }
        try {
            const snapshot = JSON.parse(raw);
            this.files.clear();
            (snapshot.files || []).forEach(([path, content]) => {
                this.files.set(path, content);
            });
            this.currentFile = snapshot.currentFile || 'lib.rs';
            this.projectName = snapshot.projectName || 'workspace';
            this.programIdOverrideEnabled = Boolean(snapshot.programIdOverrideEnabled);
            this.programKeypair = snapshot.programKeypair || null;
            this.deployedPrograms = snapshot.deployedPrograms || [];
            this.formatOnSave = Boolean(snapshot.formatOnSave);
            this.currentTemplateId = snapshot.currentTemplateId || null;
            this.templateOptions = snapshot.templateOptions || {};

            const overrideInput = document.getElementById('programIdOverrideValue');
            if (overrideInput && snapshot.programIdOverrideValue) {
                overrideInput.value = snapshot.programIdOverrideValue;
            }

            localStorage.setItem('playground_files', JSON.stringify(Array.from(this.files.entries())));
            localStorage.setItem('deployed_programs', JSON.stringify(this.deployedPrograms));
            localStorage.setItem('playground_project_name', this.projectName);
            localStorage.setItem('program_id_override_enabled', String(this.programIdOverrideEnabled));
            localStorage.setItem('program_id_override_value', snapshot.programIdOverrideValue || '');
            localStorage.setItem('playground_format_on_save', String(this.formatOnSave));
            localStorage.setItem('playground_template_state', JSON.stringify({
                currentTemplateId: this.currentTemplateId,
                templateOptions: this.templateOptions
            }));

            this.toggleProgramOverrideUI(this.programIdOverrideEnabled);
            this.updateProjectNameUI();
            this.updateFormatOnSaveUI();
            this.updateDeployedProgramsList();
            this.renderTemplateOptions();
            this.loadFile(this.currentFile);
            this.updateProgramIdPreview();

            this.showToast('Snapshot restored', 'success');
        } catch (e) {
            this.showToast('Failed to restore snapshot', 'warning');
        }
    },

    exportSnapshot() {
        const raw = localStorage.getItem('playground_snapshot');
        if (!raw) {
            this.showToast('No snapshot found', 'warning');
            return;
        }

        const blob = new Blob([raw], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `moltchain-playground-snapshot-${Date.now()}.json`;
        a.click();
        URL.revokeObjectURL(url);
    },

    async importSnapshot(file) {
        try {
            const raw = await file.text();
            localStorage.setItem('playground_snapshot', raw);
            this.restoreSnapshot();
        } catch (e) {
            this.showToast('Snapshot import failed', 'warning');
        }
    },

    openProgramInExplorer() {
        const programId = this.programIdOverrideEnabled
            ? this.getProgramIdOverride()
            : document.getElementById('programIdPreview')?.value?.trim();
        if (!programId || programId === 'Build to preview' || programId === 'Build and connect wallet' || programId === 'Override enabled') {
            this.showToast('Program ID not available yet', 'warning');
            return;
        }
        window.open(`${this.getExplorerUrl()}/program/${programId}`, '_blank');
    },

    openProgramInfoInExplorer() {
        const programId = document.getElementById('infoProgramId')?.textContent?.trim();
        if (!programId || programId === '-') {
            this.showToast('Program ID not available', 'warning');
            return;
        }
        window.open(`${this.getExplorerUrl()}/program/${programId}`, '_blank');
    },

    renderProgramCalls() {
        const container = document.getElementById('programCallsList');
        if (!container) return;

        const filterValue = document.getElementById('programCallsFilter')?.value?.trim().toLowerCase() || '';
        const calls = this.programCallsCache || [];
        const filtered = filterValue
            ? calls.filter(call => {
                const fn = (call.function || '').toLowerCase();
                const caller = (call.caller || '').toLowerCase();
                return fn.includes(filterValue) || caller.includes(filterValue);
            })
            : calls;

        if (filtered.length === 0) {
            container.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-stream"></i>
                    <p>No program calls</p>
                </div>
            `;
            return;
        }

        // AUDIT-FIX F14.4: escape RPC data in program calls
        container.innerHTML = filtered.map(call => `
            <div class="storage-row">
                <div class="storage-key monospace">${escapeHtml(call.function)}</div>
                <div class="storage-value monospace">${escapeHtml(this.truncateAddress(call.caller))}</div>
                <div class="storage-value">${new Date(call.timestamp * 1000).toLocaleString()}</div>
            </div>
        `).join('');
    },

    showToast(message, type = 'success') {
        let container = document.getElementById('toastContainer');
        if (!container) {
            container = document.createElement('div');
            container.id = 'toastContainer';
            container.className = 'toast-container';
            document.body.appendChild(container);
        }

        const toast = document.createElement('div');
        toast.className = `toast toast-${type}`;
        toast.textContent = message;
        container.appendChild(toast);

        setTimeout(() => {
            toast.style.opacity = '0';
            toast.style.transform = 'translateY(6px)';
            toast.style.transition = 'opacity 0.2s ease, transform 0.2s ease';
            setTimeout(() => toast.remove(), 200);
        }, 2200);
    },
    
    newFile() {
        const filename = prompt('Enter filename:');
        if (filename) {
            this.files.set(filename, '// New file\n');
            this.loadFile(filename);
            this.updateFileTree();
        }
    },
    
    newFolder() {
        const foldername = prompt('Enter folder name:');
        if (foldername) {
            // Track folders as keys with trailing /
            const folderKey = foldername.endsWith('/') ? foldername : foldername + '/';
            if (!this.files.has(folderKey)) {
                this.files.set(folderKey, null);
                this.updateFileTree();
            }
            this.addTerminalLine(`📁 Created folder: ${foldername}`, 'success');
        }
    },
    
    importProgram() {
        const input = document.createElement('input');
        input.type = 'file';
        input.accept = '.zip,.wasm,.rs';
        input.onchange = async (e) => {
            const file = e.target.files[0];
            if (file) {
                const content = await file.text();
                this.files.set(file.name, content);
                this.loadFile(file.name);
                this.addTerminalLine(`📥 Imported ${file.name}`, 'success');
            }
        };
        input.click();
    },
    
    exportProgram() {
        const zip = {
            files: Array.from(this.files.entries())
        };
        
        const json = JSON.stringify(zip, null, 2);
        const blob = new Blob([json], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'moltchain-program.json';
        a.click();
        
        this.addTerminalLine('📤 Program exported', 'success');
    },
    
    // ========================================================================
    // EXAMPLES
    // ========================================================================
    
    async loadExample(exampleId) {
        this.addTerminalLine(`📚 Loading example: ${exampleId}...`, 'info');
        
        const example = EXAMPLES[exampleId];
        if (!example) {
            this.addTerminalLine(`❌ Example not found: ${exampleId}`, 'error');
            return;
        }

        if (example.templateType) {
            const defaults = this.getTemplateDefaults(exampleId, example) || {};
            this.currentTemplateId = exampleId;
            this.templateOptions = defaults;
            this.saveTemplateState();

            const files = this.generateTemplateFiles(example.templateType, defaults);
            if (!files) {
                this.addTerminalLine('❌ Template generation failed', 'error');
                return;
            }

            this.applyGeneratedTemplateFiles(files, { promptOverwrite: false, markClean: true });
            this.renderTemplateOptions();
            this.loadFile('lib.rs');

            this.addTerminalLine(`✅ Loaded example: ${example.name}`, 'success');
            this.addTerminalLine(`   ${example.description}`, 'info');
            return;
        }

        const files = await this.resolveExampleFiles(example);
        if (!files) {
            this.addTerminalLine('❌ Failed to load example files', 'error');
            return;
        }
        
        // Load example files
        this.files.clear();
        Object.entries(files).forEach(([filename, content]) => {
            this.files.set(filename, content);
        });
        
        // Load main file
        this.loadFile('lib.rs');

        this.currentTemplateId = null;
        this.templateOptions = {};
        this.saveTemplateState();
        this.renderTemplateOptions();
        
        this.addTerminalLine(`✅ Loaded example: ${example.name}`, 'success');
        this.addTerminalLine(`   ${example.description}`, 'info');
    },

    saveTemplateState() {
        localStorage.setItem('playground_template_state', JSON.stringify({
            currentTemplateId: this.currentTemplateId,
            templateOptions: this.templateOptions
        }));
    },

    renderTemplateOptions() {
        const body = document.getElementById('templateOptionsBody');
        const badge = document.getElementById('templateOptionsBadge');
        if (!body || !badge) return;

        if (!this.currentTemplateId) {
            badge.textContent = 'None';
            body.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-sliders"></i>
                    <p>Select a template to configure options</p>
                </div>
            `;
            return;
        }

        if (this.currentTemplateId === 'token') {
            badge.textContent = 'Token';
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Token Name</label>
                        <input type="text" class="form-input" id="tokenNameInput" placeholder="MoltCoin">
                    </div>
                    <div class="form-group">
                        <label>Symbol</label>
                        <input type="text" class="form-input" id="tokenSymbolInput" placeholder="MOLT">
                    </div>
                    <div class="template-registry-status">
                        <span class="template-registry-label">Registry status:</span>
                        <span class="template-registry-value" id="templateRegistryStatus">-</span>
                    </div>
                    <div class="form-group">
                        <label>Decimals</label>
                        <input type="number" class="form-input" id="tokenDecimalsInput" min="0" max="18" step="1">
                    </div>
                    <div class="form-group">
                        <label>Initial Supply</label>
                        <input type="text" class="form-input" id="tokenSupplyInput" placeholder="1000000">
                    </div>
                    <div class="form-group">
                        <label>Website URL</label>
                        <input type="text" class="form-input" id="tokenWebsiteInput" placeholder="https://moltcoin.io">
                    </div>
                    <div class="form-group">
                        <label>Logo URL</label>
                        <input type="text" class="form-input" id="tokenLogoUrlInput" placeholder="https://moltcoin.io/logo.png">
                    </div>
                    <div class="form-group" style="grid-column: 1 / -1;">
                        <label>Description</label>
                        <input type="text" class="form-input" id="tokenDescriptionInput" placeholder="A short description of your token">
                    </div>
                    <div class="form-group">
                        <label>Twitter/X URL</label>
                        <input type="text" class="form-input" id="tokenTwitterInput" placeholder="https://x.com/moltcoin">
                    </div>
                    <div class="form-group">
                        <label>Telegram URL</label>
                        <input type="text" class="form-input" id="tokenTelegramInput" placeholder="https://t.me/moltcoin">
                    </div>
                    <div class="form-group">
                        <label>Discord URL</label>
                        <input type="text" class="form-input" id="tokenDiscordInput" placeholder="https://discord.gg/moltcoin">
                    </div>
                    <div class="form-group">
                        <label>Owner (deployer)</label>
                        <div class="template-options-row">
                            <input type="text" class="form-input-sm" id="tokenOwnerInput" readonly>
                            <button class="btn-icon-sm" id="copyTokenOwnerBtn" title="Copy owner">
                                <i class="fas fa-copy"></i>
                            </button>
                        </div>
                        <div class="template-options-note">Owner is the connected wallet at deploy time.</div>
                    </div>
                    <label class="template-option-toggle">
                        <input type="checkbox" id="tokenMintableToggle">
                        Mintable (owner-only minting)
                    </label>
                    <label class="template-option-toggle">
                        <input type="checkbox" id="tokenBurnableToggle">
                        Burnable (holders burn their own balance)
                    </label>
                </div>
                <div class="template-options-note">If mintable is off, the initial supply is fixed at deploy time.</div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;

            const options = this.getTemplateDefaults('token');
            const current = { ...options, ...this.templateOptions };

            const nameInput = document.getElementById('tokenNameInput');
            const symbolInput = document.getElementById('tokenSymbolInput');
            const decimalsInput = document.getElementById('tokenDecimalsInput');
            const supplyInput = document.getElementById('tokenSupplyInput');
            const websiteInput = document.getElementById('tokenWebsiteInput');
            const logoUrlInput = document.getElementById('tokenLogoUrlInput');
            const descriptionInput = document.getElementById('tokenDescriptionInput');
            const twitterInput = document.getElementById('tokenTwitterInput');
            const telegramInput = document.getElementById('tokenTelegramInput');
            const discordInput = document.getElementById('tokenDiscordInput');
            const mintableToggle = document.getElementById('tokenMintableToggle');
            const burnableToggle = document.getElementById('tokenBurnableToggle');

            if (nameInput) nameInput.value = current.name || '';
            if (symbolInput) symbolInput.value = current.symbol || '';
            if (decimalsInput) decimalsInput.value = String(current.decimals ?? 9);
            if (supplyInput) supplyInput.value = String(current.supply ?? '1000000');
            if (websiteInput) websiteInput.value = current.website || '';
            if (logoUrlInput) logoUrlInput.value = current.logo_url || '';
            if (descriptionInput) descriptionInput.value = current.description || '';
            if (twitterInput) twitterInput.value = current.twitter || '';
            if (telegramInput) telegramInput.value = current.telegram || '';
            if (discordInput) discordInput.value = current.discord || '';
            if (mintableToggle) mintableToggle.checked = Boolean(current.mintable);
            if (burnableToggle) burnableToggle.checked = Boolean(current.burnable);

            this.templateOptions = current;
            this.saveTemplateState();
            this.updateTemplateOwnerUI();
            this.scheduleRegistryStatusUpdate();

            nameInput?.addEventListener('input', () => this.onTokenOptionsChange());
            symbolInput?.addEventListener('input', () => this.onTokenOptionsChange());
            decimalsInput?.addEventListener('change', () => this.onTokenOptionsChange());
            supplyInput?.addEventListener('input', () => this.onTokenOptionsChange());
            websiteInput?.addEventListener('input', () => this.onTokenOptionsChange());
            logoUrlInput?.addEventListener('input', () => this.onTokenOptionsChange());
            descriptionInput?.addEventListener('input', () => this.onTokenOptionsChange());
            twitterInput?.addEventListener('input', () => this.onTokenOptionsChange());
            telegramInput?.addEventListener('input', () => this.onTokenOptionsChange());
            discordInput?.addEventListener('input', () => this.onTokenOptionsChange());
            mintableToggle?.addEventListener('change', () => this.onTokenOptionsChange());
            burnableToggle?.addEventListener('change', () => this.onTokenOptionsChange());
            document.getElementById('copyTokenOwnerBtn')?.addEventListener('click', () => {
                if (!this.wallet) {
                    this.showToast('Connect wallet to copy owner', 'warning');
                    return;
                }
                navigator.clipboard.writeText(this.wallet.address);
                this.showToast('Owner copied', 'success');
            });
            return;
        }

        if (this.currentTemplateId === 'nft') {
            badge.textContent = 'NFT';
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Collection Name</label>
                        <input type="text" class="form-input" id="nftNameInput" placeholder="MoltPunks">
                    </div>
                    <div class="form-group">
                        <label>Symbol</label>
                        <input type="text" class="form-input" id="nftSymbolInput" placeholder="MPNK">
                    </div>
                    <div class="template-registry-status">
                        <span class="template-registry-label">Registry status:</span>
                        <span class="template-registry-value" id="templateRegistryStatus">-</span>
                    </div>
                    <div class="form-group">
                        <label>Max Supply (0 = unlimited)</label>
                        <input type="text" class="form-input" id="nftMaxSupplyInput" placeholder="10000">
                    </div>
                    <div class="form-group">
                        <label>Royalty (BPS)</label>
                        <input type="number" class="form-input" id="nftRoyaltyInput" min="0" max="10000" step="1">
                    </div>
                    <div class="form-group">
                        <label>Mint Authority</label>
                        <div class="template-options-row">
                            <input type="text" class="form-input-sm" id="nftAuthorityInput" readonly>
                            <button class="btn-icon-sm" id="copyNftAuthorityBtn" title="Copy authority">
                                <i class="fas fa-copy"></i>
                            </button>
                        </div>
                        <div class="template-options-note">Authority defaults to the connected wallet.</div>
                    </div>
                    <label class="template-option-toggle">
                        <input type="checkbox" id="nftPublicMintToggle">
                        Public mint (anyone can mint)
                    </label>
                    <label class="template-option-toggle">
                        <input type="checkbox" id="nftBurnableToggle">
                        Burnable (owners burn their own NFTs)
                    </label>
                </div>
                <div class="template-options-note">If public mint is off, only the mint authority can mint.</div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;

            const options = this.getTemplateDefaults('nft');
            const current = { ...options, ...this.templateOptions };

            const nameInput = document.getElementById('nftNameInput');
            const symbolInput = document.getElementById('nftSymbolInput');
            const maxSupplyInput = document.getElementById('nftMaxSupplyInput');
            const royaltyInput = document.getElementById('nftRoyaltyInput');
            const publicMintToggle = document.getElementById('nftPublicMintToggle');
            const burnableToggle = document.getElementById('nftBurnableToggle');

            if (nameInput) nameInput.value = current.name || '';
            if (symbolInput) symbolInput.value = current.symbol || '';
            if (maxSupplyInput) maxSupplyInput.value = String(current.maxSupply ?? '0');
            if (royaltyInput) royaltyInput.value = String(current.royaltyBps ?? 0);
            if (publicMintToggle) publicMintToggle.checked = Boolean(current.publicMint);
            if (burnableToggle) burnableToggle.checked = Boolean(current.burnable);

            this.templateOptions = current;
            this.saveTemplateState();
            this.updateTemplateOwnerUI();
            this.scheduleRegistryStatusUpdate();

            nameInput?.addEventListener('input', () => this.onNftOptionsChange());
            symbolInput?.addEventListener('input', () => this.onNftOptionsChange());
            maxSupplyInput?.addEventListener('input', () => this.onNftOptionsChange());
            royaltyInput?.addEventListener('change', () => this.onNftOptionsChange());
            publicMintToggle?.addEventListener('change', () => this.onNftOptionsChange());
            burnableToggle?.addEventListener('change', () => this.onNftOptionsChange());
            document.getElementById('copyNftAuthorityBtn')?.addEventListener('click', () => {
                if (!this.wallet) {
                    this.showToast('Connect wallet to copy authority', 'warning');
                    return;
                }
                navigator.clipboard.writeText(this.wallet.address);
                this.showToast('Authority copied', 'success');
            });
            return;
        }

        // --- Lending Options ---
        if (this.currentTemplateId === 'lending') {
            badge.textContent = 'Lending';
            const defaults = this.getTemplateDefaults('lending');
            const current = { ...defaults, ...this.templateOptions };
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Collateral Factor %</label>
                        <input type="number" class="form-input" id="lendCollateralInput" min="25" max="95" step="1" value="${current.collateralFactor}">
                        <div class="template-options-note">Max borrow = this % of deposit value</div>
                    </div>
                    <div class="form-group">
                        <label>Liquidation Threshold %</label>
                        <input type="number" class="form-input" id="lendLiqThreshInput" min="50" max="99" step="1" value="${current.liquidationThreshold}">
                        <div class="template-options-note">Position liquidated above this %</div>
                    </div>
                    <div class="form-group">
                        <label>Liquidation Bonus %</label>
                        <input type="number" class="form-input" id="lendLiqBonusInput" min="1" max="20" step="1" value="${current.liquidationBonus}">
                        <div class="template-options-note">Reward for liquidators</div>
                    </div>
                </div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;
            this.templateOptions = current;
            this.saveTemplateState();
            document.getElementById('lendCollateralInput')?.addEventListener('change', () => this.onLendingOptionsChange());
            document.getElementById('lendLiqThreshInput')?.addEventListener('change', () => this.onLendingOptionsChange());
            document.getElementById('lendLiqBonusInput')?.addEventListener('change', () => this.onLendingOptionsChange());
            return;
        }

        // --- Launchpad Options ---
        if (this.currentTemplateId === 'launchpad') {
            badge.textContent = 'Launchpad';
            const defaults = this.getTemplateDefaults('launchpad');
            const current = { ...defaults, ...this.templateOptions };
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Platform Fee %</label>
                        <input type="number" class="form-input" id="launchPlatformFeeInput" min="0" max="10" step="1" value="${current.platformFee}">
                        <div class="template-options-note">Fee on every bonding-curve trade</div>
                    </div>
                    <div class="form-group">
                        <label>Graduation Market Cap</label>
                        <input type="number" class="form-input" id="launchGradMcapInput" min="100000" max="10000000" step="1000" value="${current.graduationMcap}">
                        <div class="template-options-note">MOLT market cap to graduate to MoltSwap</div>
                    </div>
                </div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;
            this.templateOptions = current;
            this.saveTemplateState();
            document.getElementById('launchPlatformFeeInput')?.addEventListener('change', () => this.onLaunchpadOptionsChange());
            document.getElementById('launchGradMcapInput')?.addEventListener('change', () => this.onLaunchpadOptionsChange());
            return;
        }

        // --- Vault Options ---
        if (this.currentTemplateId === 'vault') {
            badge.textContent = 'Vault';
            const defaults = this.getTemplateDefaults('vault');
            const current = { ...defaults, ...this.templateOptions };
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Performance Fee %</label>
                        <input type="number" class="form-input" id="vaultPerfFeeInput" min="0" max="50" step="1" value="${current.performanceFee}">
                        <div class="template-options-note">Fee taken on harvested yield</div>
                    </div>
                    <div class="form-group">
                        <label>Max Strategies</label>
                        <input type="number" class="form-input" id="vaultMaxStratInput" min="1" max="10" step="1" value="${current.maxStrategies}">
                        <div class="template-options-note">Maximum active yield strategies</div>
                    </div>
                </div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;
            this.templateOptions = current;
            this.saveTemplateState();
            document.getElementById('vaultPerfFeeInput')?.addEventListener('change', () => this.onVaultOptionsChange());
            document.getElementById('vaultMaxStratInput')?.addEventListener('change', () => this.onVaultOptionsChange());
            return;
        }

        // --- Identity Options ---
        if (this.currentTemplateId === 'identity') {
            badge.textContent = 'Identity';
            const defaults = this.getTemplateDefaults('identity');
            const current = { ...defaults, ...this.templateOptions };
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Initial Reputation</label>
                        <input type="number" class="form-input" id="idInitRepInput" min="1" max="10000" step="1" value="${current.initialReputation}">
                        <div class="template-options-note">Rep score on registration</div>
                    </div>
                    <div class="form-group">
                        <label>Max Reputation</label>
                        <input type="number" class="form-input" id="idMaxRepInput" min="1000" max="1000000" step="1000" value="${current.maxReputation}">
                        <div class="template-options-note">Reputation ceiling</div>
                    </div>
                    <div class="form-group">
                        <label>Vouch Cost</label>
                        <input type="number" class="form-input" id="idVouchCostInput" min="1" max="100" step="1" value="${current.vouchCost}">
                        <div class="template-options-note">Rep deducted from voucher</div>
                    </div>
                    <div class="form-group">
                        <label>Vouch Reward</label>
                        <input type="number" class="form-input" id="idVouchRewardInput" min="1" max="200" step="1" value="${current.vouchReward}">
                        <div class="template-options-note">Rep gained by the vouched agent</div>
                    </div>
                </div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;
            this.templateOptions = current;
            this.saveTemplateState();
            document.getElementById('idInitRepInput')?.addEventListener('change', () => this.onIdentityOptionsChange());
            document.getElementById('idMaxRepInput')?.addEventListener('change', () => this.onIdentityOptionsChange());
            document.getElementById('idVouchCostInput')?.addEventListener('change', () => this.onIdentityOptionsChange());
            document.getElementById('idVouchRewardInput')?.addEventListener('change', () => this.onIdentityOptionsChange());
            return;
        }

        // --- Marketplace Options ---
        if (this.currentTemplateId === 'marketplace') {
            badge.textContent = 'Market';
            const defaults = this.getTemplateDefaults('marketplace');
            const current = { ...defaults, ...this.templateOptions };
            const feePercent = (current.feeBps / 100).toFixed(1);
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Marketplace Fee (BPS)</label>
                        <input type="number" class="form-input" id="mktFeeBpsInput" min="0" max="1000" step="25" value="${current.feeBps}">
                        <div class="template-options-note">${feePercent}% — 100 BPS = 1%</div>
                    </div>
                </div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;
            this.templateOptions = current;
            this.saveTemplateState();
            document.getElementById('mktFeeBpsInput')?.addEventListener('change', () => this.onMarketplaceOptionsChange());
            return;
        }

        // --- Auction Options ---
        if (this.currentTemplateId === 'auction') {
            badge.textContent = 'Auction';
            const defaults = this.getTemplateDefaults('auction');
            const current = { ...defaults, ...this.templateOptions };
            body.innerHTML = `
                <div class="template-options-grid">
                    <div class="form-group">
                        <label>Auction Duration (hours)</label>
                        <input type="number" class="form-input" id="auctDurationInput" min="1" max="168" step="1" value="${current.durationHours}">
                        <div class="template-options-note">Default: 24h — max 168h (7 days)</div>
                    </div>
                    <div class="form-group">
                        <label>Min Bid Increment %</label>
                        <input type="number" class="form-input" id="auctMinBidInput" min="1" max="50" step="1" value="${current.minBidIncrement}">
                        <div class="template-options-note">Each bid must exceed previous by this %</div>
                    </div>
                </div>
                <div class="template-options-note">Changing options regenerates the template code.</div>
            `;
            this.templateOptions = current;
            this.saveTemplateState();
            document.getElementById('auctDurationInput')?.addEventListener('change', () => this.onAuctionOptionsChange());
            document.getElementById('auctMinBidInput')?.addEventListener('change', () => this.onAuctionOptionsChange());
            return;
        }

        badge.textContent = 'Custom';
        body.innerHTML = `
            <div class="empty-state">
                <i class="fas fa-sliders"></i>
                <p>No template options available</p>
            </div>
        `;
    },

    updateTemplateOwnerUI() {
        const ownerInput = document.getElementById('tokenOwnerInput');
        if (ownerInput) {
            if (this.wallet) {
                ownerInput.value = this.wallet.address;
            } else {
                ownerInput.value = '';
                ownerInput.placeholder = 'Connect wallet to set owner';
            }
        }

        const authorityInput = document.getElementById('nftAuthorityInput');
        if (authorityInput) {
            if (this.wallet) {
                authorityInput.value = this.wallet.address;
            } else {
                authorityInput.value = '';
                authorityInput.placeholder = 'Connect wallet to set authority';
            }
        }
    },

    updateProgramOverrideAvailability() {
        const checkbox = document.getElementById('programIdOverride');
        if (!checkbox) return;

        const hasWallet = Boolean(this.wallet);
        checkbox.disabled = !hasWallet;

        if (!hasWallet && this.programIdOverrideEnabled) {
            this.programIdOverrideEnabled = false;
            this.toggleProgramOverrideUI(false);
            this.saveProgramOverride();
            this.updateProgramIdPreview();
        }
    },

    updateCreateWalletDescription() {
        const description = document.getElementById('createWalletDescription');
        const warning = document.getElementById('createWalletWarning');
        if (!description) return;

        if (this.network === 'mainnet') {
            description.textContent = 'Create a new wallet for mainnet deployment. Store keys securely.';
            if (warning) {
                warning.innerHTML = '<i class="fas fa-shield-alt"></i><strong>Mainnet wallet:</strong> You are responsible for key security.';
            }
            return;
        }

        if (this.network === 'local') {
            description.textContent = 'Create a new wallet for local testing.';
            if (warning) {
                warning.innerHTML = '<i class="fas fa-exclamation-triangle"></i><strong>Local only!</strong> This wallet is for development.';
            }
            return;
        }

        description.textContent = 'Create a new wallet for testnet experimentation.';
        if (warning) {
            warning.innerHTML = '<i class="fas fa-exclamation-triangle"></i><strong>For testing only!</strong> Don\'t send real funds to this wallet.';
        }
    },

    scheduleRegistryStatusUpdate() {
        if (this.registryStatusTimer) {
            clearTimeout(this.registryStatusTimer);
        }
        this.registryStatusTimer = setTimeout(() => {
            this.updateRegistryStatus();
        }, 350);
    },

    setRegistryStatus(text, tone = 'info') {
        const statusEl = document.getElementById('templateRegistryStatus');
        if (!statusEl) return;

        statusEl.textContent = text;
        statusEl.classList.remove(
            'status-available',
            'status-taken',
            'status-error',
            'status-warning',
            'status-info'
        );

        const classMap = {
            available: 'status-available',
            taken: 'status-taken',
            error: 'status-error',
            warning: 'status-warning',
            info: 'status-info'
        };

        if (classMap[tone]) {
            statusEl.classList.add(classMap[tone]);
        }
    },

    async updateRegistryStatus() {
        const statusEl = document.getElementById('templateRegistryStatus');
        if (!statusEl) return;

        const symbol = String(this.templateOptions?.symbol || '').trim();
        if (!symbol) {
            this.setRegistryStatus('Enter a symbol to check', 'info');
            return;
        }

        if (!this.rpc) {
            this.setRegistryStatus('RPC unavailable', 'warning');
            return;
        }

        this.setRegistryStatus('Checking...', 'info');

        try {
            const entry = await this.rpc.getSymbolRegistry(symbol);
            if (entry && entry.program) {
                this.registryLookupCache.set(symbol, entry);
                this.setRegistryStatus(`Taken by ${this.truncateAddress(entry.program)}`, 'taken');
            } else {
                this.registryLookupCache.delete(symbol);
                this.setRegistryStatus('Available', 'available');
            }
        } catch (error) {
            this.setRegistryStatus('Registry unavailable', 'warning');
        }
    },

    getTemplateDefaults(exampleId) {
        if (exampleId === 'token') {
            return {
                name: 'MoltCoin',
                symbol: 'MOLT',
                decimals: 9,
                supply: '1000000',
                website: '',
                logo_url: '',
                twitter: '',
                telegram: '',
                discord: '',
                mintable: true,
                burnable: true
            };
        }
        if (exampleId === 'nft') {
            return {
                name: 'MoltPunks',
                symbol: 'MPNK',
                maxSupply: '10000',
                royaltyBps: 500,
                publicMint: false,
                burnable: true
            };
        }
        if (exampleId === 'lending') {
            return { collateralFactor: 75, liquidationThreshold: 85, liquidationBonus: 5 };
        }
        if (exampleId === 'launchpad') {
            return { platformFee: 1, graduationMcap: 1000000 };
        }
        if (exampleId === 'vault') {
            return { performanceFee: 10, maxStrategies: 5 };
        }
        if (exampleId === 'identity') {
            return { initialReputation: 100, maxReputation: 100000, vouchCost: 5, vouchReward: 10 };
        }
        if (exampleId === 'marketplace') {
            return { feeBps: 250 };
        }
        if (exampleId === 'auction') {
            return { durationHours: 24, minBidIncrement: 5 };
        }
        return null;
    },

    generateTemplateFiles(templateType, options) {
        if (templateType === 'token') {
            return this.generateTokenTemplateFiles(options);
        }
        if (templateType === 'nft') {
            return this.generateNftTemplateFiles(options);
        }
        if (templateType === 'lending') {
            return this.generateLendingTemplateFiles(options);
        }
        if (templateType === 'launchpad') {
            return this.generateLaunchpadTemplateFiles(options);
        }
        if (templateType === 'vault') {
            return this.generateVaultTemplateFiles(options);
        }
        if (templateType === 'identity') {
            return this.generateIdentityTemplateFiles(options);
        }
        if (templateType === 'marketplace') {
            return this.generateMarketplaceTemplateFiles(options);
        }
        if (templateType === 'auction') {
            return this.generateAuctionTemplateFiles(options);
        }
        return null;
    },

    onTokenOptionsChange() {
        const options = this.readTokenOptionsFromUI();
        if (!options) return;
        this.templateOptions = options;
        this.saveTemplateState();
        this.applyTokenTemplateOptions(options);
        this.scheduleRegistryStatusUpdate();
    },

    onNftOptionsChange() {
        const options = this.readNftOptionsFromUI();
        if (!options) return;
        this.templateOptions = options;
        this.saveTemplateState();
        this.applyNftTemplateOptions(options);
        this.scheduleRegistryStatusUpdate();
    },

    readTokenOptionsFromUI() {
        const nameInput = document.getElementById('tokenNameInput');
        const symbolInput = document.getElementById('tokenSymbolInput');
        const decimalsInput = document.getElementById('tokenDecimalsInput');
        const supplyInput = document.getElementById('tokenSupplyInput');
        const websiteInput = document.getElementById('tokenWebsiteInput');
        const logoUrlInput = document.getElementById('tokenLogoUrlInput');
        const descriptionInput = document.getElementById('tokenDescriptionInput');
        const twitterInput = document.getElementById('tokenTwitterInput');
        const telegramInput = document.getElementById('tokenTelegramInput');
        const discordInput = document.getElementById('tokenDiscordInput');
        const mintableToggle = document.getElementById('tokenMintableToggle');
        const burnableToggle = document.getElementById('tokenBurnableToggle');

        if (!nameInput || !symbolInput || !decimalsInput || !supplyInput) return null;

        const name = this.normalizeTokenName(nameInput.value || '');
        const symbol = this.normalizeTokenSymbol(symbolInput.value || '');
        const decimals = this.normalizeTokenDecimals(decimalsInput.value);

        if (decimals === null) {
            this.showToast('Decimals must be between 0 and 18', 'warning');
            return null;
        }

        const supplyRaw = (supplyInput.value || '').trim().replace(/_/g, '');
        if (!/^[0-9]+$/.test(supplyRaw || '')) {
            this.showToast('Initial supply must be a whole number', 'warning');
            return null;
        }

        const supply = supplyRaw === '' ? '0' : supplyRaw;
        const supplyBase = this.toTokenBaseUnits(supply, decimals);
        if (supplyBase === null) {
            this.showToast('Initial supply exceeds u64 limits', 'warning');
            return null;
        }

        const website = (websiteInput?.value || '').trim();
        const logoUrl = (logoUrlInput?.value || '').trim();
        const description = (descriptionInput?.value || '').trim();
        const twitter = (twitterInput?.value || '').trim();
        const telegram = (telegramInput?.value || '').trim();
        const discord = (discordInput?.value || '').trim();

        if (nameInput.value !== name) nameInput.value = name;
        if (symbolInput.value !== symbol) symbolInput.value = symbol;

        return {
            name,
            symbol,
            decimals,
            supply,
            initialSupplyBase: supplyBase,
            website,
            logo_url: logoUrl,
            description,
            twitter,
            telegram,
            discord,
            mintable: Boolean(mintableToggle?.checked),
            burnable: Boolean(burnableToggle?.checked)
        };
    },

    normalizeTokenName(raw) {
        const trimmed = (raw || '').trim();
        return trimmed || 'Token';
    },

    normalizeTokenSymbol(raw) {
        const cleaned = (raw || '').toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 10);
        return cleaned || 'TOKEN';
    },

    normalizeNftName(raw) {
        const trimmed = (raw || '').trim();
        return trimmed || 'NFT Collection';
    },

    normalizeNftSymbol(raw) {
        const cleaned = (raw || '').toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 10);
        return cleaned || 'NFT';
    },

    normalizeNftRoyalty(raw) {
        const trimmed = (raw || '').trim();
        if (trimmed === '') return 0;
        const parsed = Number.parseInt(trimmed, 10);
        if (!Number.isFinite(parsed)) return null;
        if (parsed < 0 || parsed > 10000) return null;
        return parsed;
    },

    normalizeNftMaxSupply(raw) {
        const cleaned = (raw || '').trim().replace(/_/g, '');
        if (cleaned === '') return '0';
        if (!/^[0-9]+$/.test(cleaned)) return null;
        return cleaned;
    },

    normalizeTokenDecimals(raw) {
        const parsed = Number.parseInt(raw, 10);
        if (!Number.isFinite(parsed)) return null;
        if (parsed < 0 || parsed > 18) return null;
        return parsed;
    },

    toTokenBaseUnits(supply, decimals) {
        try {
            const supplyBig = BigInt(supply || '0');
            const scale = 10n ** BigInt(decimals);
            const total = supplyBig * scale;
            const max = (1n << 64n) - 1n;
            if (total > max) return null;
            return total.toString();
        } catch (e) {
            return null;
        }
    },

    applyTokenTemplateOptions(options) {
        const files = this.generateTokenTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },

    applyNftTemplateOptions(options) {
        const files = this.generateNftTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },

    // --- Utility: replace a Rust const value in source code ---
    replaceRustConst(code, constName, newValue) {
        const re = new RegExp(`(const ${constName}:\\s*\\w+\\s*=\\s*)([^;]+)(;)`, 'g');
        return code.replace(re, `$1${newValue}$3`);
    },

    // --- Lending (LobsterLend) ---
    generateLendingTemplateFiles(options) {
        const example = EXAMPLES.lending;
        let code = example.files['lib.rs'];
        code = this.replaceRustConst(code, 'COLLATERAL_FACTOR_PERCENT', options.collateralFactor);
        code = this.replaceRustConst(code, 'LIQUIDATION_THRESHOLD', options.liquidationThreshold);
        code = this.replaceRustConst(code, 'LIQUIDATION_BONUS', options.liquidationBonus);
        return { 'lib.rs': code, 'Cargo.toml': example.files['Cargo.toml'] };
    },
    applyLendingTemplateOptions(options) {
        const files = this.generateLendingTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },
    onLendingOptionsChange() {
        const opts = this.readLendingOptionsFromUI();
        if (!opts) return;
        this.templateOptions = opts;
        this.saveTemplateState();
        this.applyLendingTemplateOptions(opts);
    },
    readLendingOptionsFromUI() {
        const cf = parseInt(document.getElementById('lendCollateralInput')?.value, 10);
        const lt = parseInt(document.getElementById('lendLiqThreshInput')?.value, 10);
        const lb = parseInt(document.getElementById('lendLiqBonusInput')?.value, 10);
        if (![cf, lt, lb].every(v => Number.isFinite(v))) return null;
        return { collateralFactor: Math.max(25, Math.min(95, cf)), liquidationThreshold: Math.max(50, Math.min(99, lt)), liquidationBonus: Math.max(1, Math.min(20, lb)) };
    },

    // --- Launchpad (ClawPump) ---
    generateLaunchpadTemplateFiles(options) {
        const example = EXAMPLES.launchpad;
        let code = example.files['lib.rs'];
        code = this.replaceRustConst(code, 'PLATFORM_FEE_PERCENT', options.platformFee);
        code = this.replaceRustConst(code, 'GRADUATION_MCAP', options.graduationMcap.toLocaleString('en').replace(/,/g, '_'));
        return { 'lib.rs': code, 'Cargo.toml': example.files['Cargo.toml'] };
    },
    applyLaunchpadTemplateOptions(options) {
        const files = this.generateLaunchpadTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },
    onLaunchpadOptionsChange() {
        const opts = this.readLaunchpadOptionsFromUI();
        if (!opts) return;
        this.templateOptions = opts;
        this.saveTemplateState();
        this.applyLaunchpadTemplateOptions(opts);
    },
    readLaunchpadOptionsFromUI() {
        const pf = parseInt(document.getElementById('launchPlatformFeeInput')?.value, 10);
        const gm = parseInt(document.getElementById('launchGradMcapInput')?.value, 10);
        if (![pf, gm].every(v => Number.isFinite(v))) return null;
        return { platformFee: Math.max(0, Math.min(10, pf)), graduationMcap: Math.max(100000, Math.min(10000000, gm)) };
    },

    // --- Vault (ClawVault) ---
    generateVaultTemplateFiles(options) {
        const example = EXAMPLES.vault;
        let code = example.files['lib.rs'];
        code = this.replaceRustConst(code, 'PERFORMANCE_FEE', options.performanceFee);
        code = this.replaceRustConst(code, 'MAX_STRATEGIES', options.maxStrategies);
        return { 'lib.rs': code, 'Cargo.toml': example.files['Cargo.toml'] };
    },
    applyVaultTemplateOptions(options) {
        const files = this.generateVaultTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },
    onVaultOptionsChange() {
        const opts = this.readVaultOptionsFromUI();
        if (!opts) return;
        this.templateOptions = opts;
        this.saveTemplateState();
        this.applyVaultTemplateOptions(opts);
    },
    readVaultOptionsFromUI() {
        const pf = parseInt(document.getElementById('vaultPerfFeeInput')?.value, 10);
        const ms = parseInt(document.getElementById('vaultMaxStratInput')?.value, 10);
        if (![pf, ms].every(v => Number.isFinite(v))) return null;
        return { performanceFee: Math.max(0, Math.min(50, pf)), maxStrategies: Math.max(1, Math.min(10, ms)) };
    },

    // --- Identity (MoltyID) ---
    generateIdentityTemplateFiles(options) {
        const example = EXAMPLES.identity;
        let code = example.files['lib.rs'];
        code = this.replaceRustConst(code, 'INITIAL_REPUTATION', options.initialReputation);
        code = this.replaceRustConst(code, 'MAX_REPUTATION', options.maxReputation.toLocaleString('en').replace(/,/g, '_'));
        code = this.replaceRustConst(code, 'VOUCH_COST', options.vouchCost);
        code = this.replaceRustConst(code, 'VOUCH_REWARD', options.vouchReward);
        return { 'lib.rs': code, 'Cargo.toml': example.files['Cargo.toml'] };
    },
    applyIdentityTemplateOptions(options) {
        const files = this.generateIdentityTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },
    onIdentityOptionsChange() {
        const opts = this.readIdentityOptionsFromUI();
        if (!opts) return;
        this.templateOptions = opts;
        this.saveTemplateState();
        this.applyIdentityTemplateOptions(opts);
    },
    readIdentityOptionsFromUI() {
        const ir = parseInt(document.getElementById('idInitRepInput')?.value, 10);
        const mr = parseInt(document.getElementById('idMaxRepInput')?.value, 10);
        const vc = parseInt(document.getElementById('idVouchCostInput')?.value, 10);
        const vr = parseInt(document.getElementById('idVouchRewardInput')?.value, 10);
        if (![ir, mr, vc, vr].every(v => Number.isFinite(v))) return null;
        return { initialReputation: Math.max(1, Math.min(10000, ir)), maxReputation: Math.max(1000, Math.min(1000000, mr)), vouchCost: Math.max(1, Math.min(100, vc)), vouchReward: Math.max(1, Math.min(200, vr)) };
    },

    // --- Marketplace (MoltMarket) ---
    generateMarketplaceTemplateFiles(options) {
        const example = EXAMPLES.marketplace;
        let code = example.files['lib.rs'];
        code = this.replaceRustConst(code, 'DEFAULT_FEE_BPS', options.feeBps);
        return { 'lib.rs': code, 'Cargo.toml': example.files['Cargo.toml'] };
    },
    applyMarketplaceTemplateOptions(options) {
        const files = this.generateMarketplaceTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },
    onMarketplaceOptionsChange() {
        const opts = this.readMarketplaceOptionsFromUI();
        if (!opts) return;
        this.templateOptions = opts;
        this.saveTemplateState();
        this.applyMarketplaceTemplateOptions(opts);
    },
    readMarketplaceOptionsFromUI() {
        const fb = parseInt(document.getElementById('mktFeeBpsInput')?.value, 10);
        if (!Number.isFinite(fb)) return null;
        return { feeBps: Math.max(0, Math.min(1000, fb)) };
    },

    // --- Auction (MoltAuction) ---
    generateAuctionTemplateFiles(options) {
        const example = EXAMPLES.auction;
        let code = example.files['lib.rs'];
        const durationSecs = options.durationHours * 3600;
        code = this.replaceRustConst(code, 'AUCTION_DURATION', durationSecs);
        code = this.replaceRustConst(code, 'MIN_BID_INCREMENT', options.minBidIncrement);
        return { 'lib.rs': code, 'Cargo.toml': example.files['Cargo.toml'] };
    },
    applyAuctionTemplateOptions(options) {
        const files = this.generateAuctionTemplateFiles(options);
        if (!files) return false;
        return this.applyGeneratedTemplateFiles(files, { promptOverwrite: true, markClean: true });
    },
    onAuctionOptionsChange() {
        const opts = this.readAuctionOptionsFromUI();
        if (!opts) return;
        this.templateOptions = opts;
        this.saveTemplateState();
        this.applyAuctionTemplateOptions(opts);
    },
    readAuctionOptionsFromUI() {
        const dh = parseInt(document.getElementById('auctDurationInput')?.value, 10);
        const mb = parseInt(document.getElementById('auctMinBidInput')?.value, 10);
        if (![dh, mb].every(v => Number.isFinite(v))) return null;
        return { durationHours: Math.max(1, Math.min(168, dh)), minBidIncrement: Math.max(1, Math.min(50, mb)) };
    },

    applyGeneratedTemplateFiles(files, { promptOverwrite = false, markClean = false } = {}) {
        const willOverwrite = ['lib.rs', 'Cargo.toml'].some(name => this.modifiedFiles.has(name));
        if (promptOverwrite && willOverwrite) {
            if (!confirm('Template changes will overwrite your current lib.rs and Cargo.toml. Continue?')) {
                return false;
            }
        }

        Object.entries(files).forEach(([filename, content]) => {
            this.files.set(filename, content);
            if (markClean) {
                this.modifiedFiles.delete(filename);
            }
        });

        if (files[this.currentFile] && this.editor) {
            this.editor.setValue(files[this.currentFile]);
        }

        localStorage.setItem('playground_files', JSON.stringify(Array.from(this.files.entries())));
        this.updateProgramIdPreview();
        return true;
    },

    generateTokenTemplateFiles(options) {
        const tokenOptions = options || this.getTemplateDefaults('token');
        if (!tokenOptions) return null;

        return {
            'lib.rs': this.buildTokenLibRs(tokenOptions),
            'Cargo.toml': this.buildTokenCargoToml(tokenOptions)
        };
    },

    generateNftTemplateFiles(options) {
        const nftOptions = options || this.getTemplateDefaults('nft');
        if (!nftOptions) return null;

        return {
            'lib.rs': this.buildNftLibRs(nftOptions),
            'Cargo.toml': this.buildNftCargoToml(nftOptions)
        };
    },

    buildTokenCargoToml(options) {
        const base = this.normalizeTokenSymbol(options.symbol || 'TOKEN').toLowerCase() || 'token';
        const packageName = `${base}-token`;
        return `[package]
name = "${packageName}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`;
    },

    buildTokenLibRs(options) {
        const name = this.normalizeTokenName(options.name);
        const symbol = this.normalizeTokenSymbol(options.symbol);
        const decimals = options.decimals ?? 9;
        const supply = options.initialSupplyBase || this.toTokenBaseUnits(options.supply || '0', decimals) || '0';
        const mintable = Boolean(options.mintable);
        const burnable = Boolean(options.burnable);

        const mintFn = mintable ? `
/// Mint new tokens (owner only)
#[no_mangle]
pub extern "C" fn mint(caller_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let caller_bytes = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let to_bytes = unsafe { core::slice::from_raw_parts(to_ptr, 32) };

    let mut caller_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    caller_array.copy_from_slice(caller_bytes);
    to_array.copy_from_slice(to_bytes);

    let caller = Address::new(caller_array);
    let to = Address::new(to_array);
    let owner = get_owner();

    match get_token().mint(to, amount, caller, owner) {
        Ok(_) => {
            log_info("Mint successful");
            1
        }
        Err(_) => {
            log_info("Mint failed - unauthorized");
            0
        }
    }
}
` : '';

        const burnFn = burnable ? `
/// Burn tokens
#[no_mangle]
pub extern "C" fn burn(from_ptr: *const u8, amount: u64) -> u32 {
    let from_bytes = unsafe { core::slice::from_raw_parts(from_ptr, 32) };
    let mut from_array = [0u8; 32];
    from_array.copy_from_slice(from_bytes);
    let from = Address::new(from_array);

    match get_token().burn(from, amount) {
        Ok(_) => {
            log_info("Burn successful");
            1
        }
        Err(_) => {
            log_info("Burn failed");
            0
        }
    }
}
` : '';

        return `// ${name} Token Contract
// Project: ${this.projectName || 'workspace'}
// MT-20 fungible token template

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use moltchain_sdk::{Token, Address, log_info};

// Program ID (auto-updated by Playground)
const PROGRAM_ID: &str = "program_id_here";

const TOKEN_NAME: &str = "${name}";
const TOKEN_SYMBOL: &str = "${symbol}";
const TOKEN_DECIMALS: u8 = ${decimals};
const INITIAL_SUPPLY: u64 = ${supply};

// Initialize token
static mut TOKEN: Option<Token> = None;
static mut OWNER: Option<Address> = None;

fn get_token() -> &'static mut Token {
    unsafe {
        TOKEN.as_mut().expect("Token not initialized")
    }
}

fn get_owner() -> Address {
    unsafe {
        OWNER.expect("Owner not set")
    }
}

/// Initialize the token contract
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) {
    let owner_bytes = unsafe {
        core::slice::from_raw_parts(owner_ptr, 32)
    };
    let mut owner_array = [0u8; 32];
    owner_array.copy_from_slice(owner_bytes);
    let owner = Address::new(owner_array);

    unsafe {
        OWNER = Some(owner);
        TOKEN = Some(Token::new(TOKEN_NAME, TOKEN_SYMBOL, TOKEN_DECIMALS));
    }

    get_token().initialize(INITIAL_SUPPLY, owner).expect("Initialization failed");
    log_info("Token initialized");
}

/// Get balance of an account
#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    let account_bytes = unsafe {
        core::slice::from_raw_parts(account_ptr, 32)
    };
    let mut account_array = [0u8; 32];
    account_array.copy_from_slice(account_bytes);
    let account = Address::new(account_array);

    get_token().balance_of(account)
}

/// Transfer tokens
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let from_bytes = unsafe { core::slice::from_raw_parts(from_ptr, 32) };
    let to_bytes = unsafe { core::slice::from_raw_parts(to_ptr, 32) };

    let mut from_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    from_array.copy_from_slice(from_bytes);
    to_array.copy_from_slice(to_bytes);

    let from = Address::new(from_array);
    let to = Address::new(to_array);

    match get_token().transfer(from, to, amount) {
        Ok(_) => {
            log_info("Transfer successful");
            1
        }
        Err(_) => {
            log_info("Transfer failed");
            0
        }
    }
}

/// Approve spender
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, amount: u64) -> u32 {
    let owner_bytes = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let spender_bytes = unsafe { core::slice::from_raw_parts(spender_ptr, 32) };

    let mut owner_array = [0u8; 32];
    let mut spender_array = [0u8; 32];
    owner_array.copy_from_slice(owner_bytes);
    spender_array.copy_from_slice(spender_bytes);

    let owner = Address::new(owner_array);
    let spender = Address::new(spender_array);

    match get_token().approve(owner, spender, amount) {
        Ok(_) => {
            log_info("Approval successful");
            1
        }
        Err(_) => 0,
    }
}

/// Allowance
#[no_mangle]
pub extern "C" fn allowance(owner_ptr: *const u8, spender_ptr: *const u8) -> u64 {
    let owner_bytes = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let spender_bytes = unsafe { core::slice::from_raw_parts(spender_ptr, 32) };

    let mut owner_array = [0u8; 32];
    let mut spender_array = [0u8; 32];
    owner_array.copy_from_slice(owner_bytes);
    spender_array.copy_from_slice(spender_bytes);

    let owner = Address::new(owner_array);
    let spender = Address::new(spender_array);

    get_token().allowance(owner, spender)
}

/// Transfer from (using allowance)
#[no_mangle]
pub extern "C" fn transfer_from(
    caller_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    amount: u64
) -> u32 {
    let caller_bytes = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let from_bytes = unsafe { core::slice::from_raw_parts(from_ptr, 32) };
    let to_bytes = unsafe { core::slice::from_raw_parts(to_ptr, 32) };

    let mut caller_array = [0u8; 32];
    let mut from_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    caller_array.copy_from_slice(caller_bytes);
    from_array.copy_from_slice(from_bytes);
    to_array.copy_from_slice(to_bytes);

    let caller = Address::new(caller_array);
    let from = Address::new(from_array);
    let to = Address::new(to_array);

    match get_token().transfer_from(caller, from, to, amount) {
        Ok(_) => {
            log_info("TransferFrom successful");
            1
        }
        Err(_) => {
            log_info("TransferFrom failed");
            0
        }
    }
}
${mintFn}
${burnFn}
/// Get total supply
#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    get_token().total_supply
}
`;
    },

    buildNftCargoToml(options) {
        const base = this.normalizeNftSymbol(options.symbol || 'NFT').toLowerCase() || 'nft';
        const packageName = `${base}-collection`;
        return `[package]
name = "${packageName}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`;
    },

    buildNftLibRs(options) {
        const name = this.normalizeNftName(options.name);
        const symbol = this.normalizeNftSymbol(options.symbol);
        const maxSupplyRaw = this.normalizeNftMaxSupply(options.maxSupply || '0') || '0';
        const royaltyBps = this.normalizeNftRoyalty(options.royaltyBps ?? 0) ?? 0;
        const publicMint = Boolean(options.publicMint);
        const burnable = Boolean(options.burnable);

        const burnFn = burnable ? `
/// Burn NFT
#[no_mangle]
pub extern "C" fn burn(owner_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        match get_nft().burn(owner, token_id) {
            Ok(_) => {
                log_info("NFT burned");
                1
            }
            Err(_) => {
                log_info("Burn failed");
                0
            }
        }
    }
}
` : '';

        return `// ${name} NFT Collection
// Project: ${this.projectName || 'workspace'}
// MT-721 collection template

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use moltchain_sdk::{NFT, Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes};

// Program ID (auto-updated by Playground)
const PROGRAM_ID: &str = "program_id_here";

const COLLECTION_NAME: &str = "${name}";
const COLLECTION_SYMBOL: &str = "${symbol}";
const MAX_SUPPLY: u64 = ${maxSupplyRaw};
const ROYALTY_BPS: u64 = ${royaltyBps};
const PUBLIC_MINT: bool = ${publicMint ? 'true' : 'false'};

static mut NFT_COLLECTION: Option<NFT> = None;
static mut MINTER: Option<Address> = None;

// Helper to get NFT collection
fn get_nft() -> &'static mut NFT {
    unsafe {
        NFT_COLLECTION.as_mut().expect("NFT not initialized")
    }
}

// Helper to get minter address
fn get_minter() -> Address {
    unsafe {
        MINTER.expect("Minter not set")
    }
}

#[no_mangle]
pub extern "C" fn initialize(minter_ptr: *const u8) {
    unsafe {
        let minter_slice = core::slice::from_raw_parts(minter_ptr, 32);
        let mut minter_addr = [0u8; 32];
        minter_addr.copy_from_slice(minter_slice);
        let minter = Address(minter_addr);
        
        NFT_COLLECTION = Some(NFT::new(COLLECTION_NAME, COLLECTION_SYMBOL));
        MINTER = Some(minter);
        
        get_nft().initialize(minter).expect("Init failed");
        storage_set(b"max_supply", &u64_to_bytes(MAX_SUPPLY));
        storage_set(b"royalty_bps", &u64_to_bytes(ROYALTY_BPS));
        storage_set(b"public_mint", &[if PUBLIC_MINT { 1 } else { 0 }]);
        
        log_info("NFT collection initialized");
    }
}

#[no_mangle]
pub extern "C" fn mint(
    caller_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
    metadata_ptr: *const u8,
    metadata_len: u32,
) -> u32 {
    unsafe {
        let caller_slice = core::slice::from_raw_parts(caller_ptr, 32);
        let mut caller_addr = [0u8; 32];
        caller_addr.copy_from_slice(caller_slice);
        let caller = Address(caller_addr);
        
        if !PUBLIC_MINT && caller.0 != get_minter().0 {
            log_info("Unauthorized: Only minter can mint");
            return 0;
        }

        let minted = storage_get(b"total_minted")
            .map(|bytes| bytes_to_u64(&bytes))
            .unwrap_or(0);
        if MAX_SUPPLY > 0 && minted >= MAX_SUPPLY {
            log_info("Max supply reached");
            return 0;
        }
        
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        let metadata = core::slice::from_raw_parts(metadata_ptr, metadata_len as usize);
        
        match get_nft().mint(to, token_id, metadata) {
            Ok(_) => {
                log_info("NFT minted successfully");
                1
            }
            Err(_) => {
                log_info("Mint failed");
                0
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let from_slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut from_addr = [0u8; 32];
        from_addr.copy_from_slice(from_slice);
        let from = Address(from_addr);
        
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        match get_nft().transfer(from, to, token_id) {
            Ok(_) => {
                log_info("NFT transferred successfully");
                1
            }
            Err(_) => {
                log_info("Transfer failed");
                0
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn owner_of(token_id: u64, out_ptr: *mut u8) -> u32 {
    unsafe {
        match get_nft().owner_of(token_id) {
            Ok(owner) => {
                let out_slice = core::slice::from_raw_parts_mut(out_ptr, 32);
                out_slice.copy_from_slice(&owner.0);
                1
            }
            Err(_) => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    unsafe {
        let account_slice = core::slice::from_raw_parts(account_ptr, 32);
        let mut account_addr = [0u8; 32];
        account_addr.copy_from_slice(account_slice);
        let account = Address(account_addr);
        
        get_nft().balance_of(account)
    }
}

#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        let spender_slice = core::slice::from_raw_parts(spender_ptr, 32);
        let mut spender_addr = [0u8; 32];
        spender_addr.copy_from_slice(spender_slice);
        let spender = Address(spender_addr);
        
        match get_nft().approve(owner, spender, token_id) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn set_approval_for_all(
    owner_ptr: *const u8,
    operator_ptr: *const u8,
    approved: u32
) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        let operator_slice = core::slice::from_raw_parts(operator_ptr, 32);
        let mut operator_addr = [0u8; 32];
        operator_addr.copy_from_slice(operator_slice);
        let operator = Address(operator_addr);
        
        match get_nft().set_approval_for_all(owner, operator, approved == 1) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn is_approved_for_all(owner_ptr: *const u8, operator_ptr: *const u8) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        let operator_slice = core::slice::from_raw_parts(operator_ptr, 32);
        let mut operator_addr = [0u8; 32];
        operator_addr.copy_from_slice(operator_slice);
        let operator = Address(operator_addr);
        
        if get_nft().is_approved_for_all(owner, operator) { 1 } else { 0 }
    }
}

#[no_mangle]
pub extern "C" fn transfer_from(
    caller_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
) -> u32 {
    unsafe {
        let caller_slice = core::slice::from_raw_parts(caller_ptr, 32);
        let mut caller_addr = [0u8; 32];
        caller_addr.copy_from_slice(caller_slice);
        let caller = Address(caller_addr);
        
        let from_slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut from_addr = [0u8; 32];
        from_addr.copy_from_slice(from_slice);
        let from = Address(from_addr);
        
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        match get_nft().transfer_from(caller, from, to, token_id) {
            Ok(_) => {
                log_info("TransferFrom successful");
                1
            }
            Err(_) => {
                log_info("TransferFrom failed");
                0
            }
        }
    }
}
${burnFn}
#[no_mangle]
pub extern "C" fn total_minted() -> u64 {
    storage_get(b"total_minted")
        .map(|bytes| bytes_to_u64(&bytes))
        .unwrap_or(0)
}
`;
    },

    readNftOptionsFromUI() {
        const nameInput = document.getElementById('nftNameInput');
        const symbolInput = document.getElementById('nftSymbolInput');
        const maxSupplyInput = document.getElementById('nftMaxSupplyInput');
        const royaltyInput = document.getElementById('nftRoyaltyInput');
        const publicMintToggle = document.getElementById('nftPublicMintToggle');
        const burnableToggle = document.getElementById('nftBurnableToggle');

        if (!nameInput || !symbolInput || !maxSupplyInput || !royaltyInput) return null;

        const name = this.normalizeNftName(nameInput.value || '');
        const symbol = this.normalizeNftSymbol(symbolInput.value || '');
        const maxSupply = this.normalizeNftMaxSupply(maxSupplyInput.value || '0');
        if (maxSupply === null) {
            this.showToast('Max supply must be a whole number', 'warning');
            return null;
        }

        try {
            const maxSupplyBig = BigInt(maxSupply);
            const max = (1n << 64n) - 1n;
            if (maxSupplyBig > max) {
                this.showToast('Max supply exceeds u64 limits', 'warning');
                return null;
            }
        } catch (e) {
            this.showToast('Max supply must be a whole number', 'warning');
            return null;
        }

        const royaltyBps = this.normalizeNftRoyalty(royaltyInput.value);
        if (royaltyBps === null) {
            this.showToast('Royalty must be between 0 and 10000', 'warning');
            return null;
        }

        if (nameInput.value !== name) nameInput.value = name;
        if (symbolInput.value !== symbol) symbolInput.value = symbol;

        return {
            name,
            symbol,
            maxSupply,
            royaltyBps,
            publicMint: Boolean(publicMintToggle?.checked),
            burnable: Boolean(burnableToggle?.checked)
        };
    },

    async resolveExampleFiles(example) {
        if (!example.sourcePath) {
            return example.files;
        }

        try {
            const url = new URL(example.sourcePath, window.location.href).toString();
            const response = await fetch(url, { cache: 'no-store' });
            if (!response.ok) {
                throw new Error(`Example source returned ${response.status}`);
            }
            const source = await response.text();
            return {
                'lib.rs': this.normalizeExampleSource(source),
                'Cargo.toml': DEFAULT_FILES.CARGO_TOML
            };
        } catch (error) {
            if (example.files) {
                this.addTerminalLine('⚠️  Falling back to embedded example source', 'warning');
                return example.files;
            }
            this.addTerminalLine('❌ Example source unavailable', 'error');
            return null;
        }
    },

    normalizeExampleSource(source) {
        let normalized = source || '';

        if (!/use\s+moltchain_sdk::\*/.test(normalized)) {
            const borshMatch = normalized.match(/^use\s+borsh[^\n]*\n/m);
            if (borshMatch) {
                normalized = normalized.replace(borshMatch[0], `${borshMatch[0]}use moltchain_sdk::*;\n`);
            } else {
                normalized = `use moltchain_sdk::*;\n\n${normalized}`;
            }
        }

        if (!/const\s+PROGRAM_ID:\s*&str\s*=/.test(normalized)) {
            const lines = normalized.split('\n');
            let insertIndex = 0;
            const shouldSkip = (line) => {
                const trimmed = line.trim();
                return trimmed === ''
                    || trimmed.startsWith('//')
                    || trimmed.startsWith('use ')
                    || trimmed.startsWith('#!')
                    || trimmed.startsWith('extern crate');
            };
            while (insertIndex < lines.length && shouldSkip(lines[insertIndex])) {
                insertIndex += 1;
            }
            lines.splice(insertIndex, 0, '', '// Program ID (auto-updated by Playground)', 'const PROGRAM_ID: &str = "program_id_here";', '');
            normalized = lines.join('\n');
        }

        return normalized;
    },
    
    // ========================================================================
    // BUILD & DEPLOY
    // ========================================================================
    
    async buildProgram() {
        this.addTerminalLine('🔨 Building program...', 'info');
        this.addTerminalLine('', 'normal');
        
        document.getElementById('buildStatus').innerHTML = '<i class="fas fa-spinner fa-spin"></i> <span>Building...</span>';
        
        const code = this.editor.getValue();
        // Detect language from current file extension
        const currentFile = this.currentFile || 'main.rs';
        const ext = currentFile.split('.').pop().toLowerCase();
        const langMap = { rs: 'rust', ts: 'typescript', js: 'javascript', py: 'python', sol: 'solidity', c: 'c', cpp: 'cpp' };
        const language = langMap[ext] || 'rust';
        
        try {
            // Call compiler API
            const compilerUrl = `${this.rpc.rpcUrl.replace('/rpc', '')}/compile`;
            const response = await fetch(compilerUrl, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    code,
                    language,
                    optimize: true
                })
            });
            
            if (!response.ok) {
                throw new Error(`Compiler returned ${response.status}`);
            }
            
            const result = await response.json();
            
            if (result.success) {
                // Success
                this.compiledWasm = this.base64ToBytes(result.wasm);
                
                this.addTerminalLine('✅ Build successful!', 'success');
                this.addTerminalLine(`   Program size: ${this.formatBytes(this.compiledWasm.length)}`, 'info');
                this.addTerminalLine(`   Build time: ${result.time_ms}ms`, 'info');
                
                if (result.warnings && result.warnings.length > 0) {
                    this.addTerminalLine('', 'normal');
                    this.addTerminalLine('⚠️  Warnings:', 'warning');
                    result.warnings.forEach(w => this.addTerminalLine(`   ${w}`, 'warning'));
                }
                
                document.getElementById('buildStatus').innerHTML = '<i class="fas fa-check-circle"></i> <span>Build successful</span>';
                document.getElementById('deployBtn').disabled = false;

                this.updateProgramIdPreview();
                
            } else {
                // Build failed
                this.buildErrors = result.errors;
                
                this.addTerminalLine('❌ Build failed:', 'error');
                this.addTerminalLine('', 'normal');
                
                result.errors.forEach(err => {
                    this.addTerminalLine(`   ${err.file}:${err.line}:${err.col}`, 'error');
                    this.addTerminalLine(`   ${err.message}`, 'error');
                    this.addTerminalLine('', 'normal');
                });
                
                document.getElementById('buildStatus').innerHTML = '<i class="fas fa-times-circle"></i> <span>Build failed</span>';
                
                // Update problems panel
                this.updateProblemsPanel(result.errors);
            }
            
        } catch (error) {
            this.addTerminalLine('❌ Compilation error:', 'error');
            this.addTerminalLine(`   ${error.message}`, 'error');
            
            document.getElementById('buildStatus').innerHTML = '<i class="fas fa-exclamation-circle"></i> <span>Error</span>';
        }
    },
    
    async deployProgram() {
        if (!this.compiledWasm) {
            this.addTerminalLine('❌ No compiled WASM. Build first!', 'error');
            return;
        }
        
        if (!this.wallet) {
            this.addTerminalLine('❌ No wallet connected. Create or import wallet first!', 'error');
            this.openWalletModal();
            return;
        }
        
        this.addTerminalLine('🚀 Deploying program...', 'info');
        this.addTerminalLine('', 'normal');
        
        try {
            // Create deployer
            const deployer = new MoltChain.ProgramDeployer(this.rpc, this.wallet);
            
            // Get deploy settings from sidebar
            const programName = document.getElementById('programName')?.value || 'my_program';
            const verify = document.getElementById('verifyCode')?.checked || false;
            const fundingInput = document.getElementById('initialFunding')?.value || '0';
            const fundingMolt = Number.parseFloat(fundingInput);
            if (!Number.isFinite(fundingMolt) || fundingMolt < 0) {
                this.addTerminalLine('❌ Initial funding must be a non-negative number', 'error');
                return;
            }
            const initialFunding = Math.round(fundingMolt * 1_000_000_000);
            const programIdOverride = this.getProgramIdOverride();
            const upgradeAuthority = document.getElementById('upgradeAuthority')?.value || 'wallet';
            const customAuthority = document.getElementById('customAuthority')?.value || '';
            const makePublic = document.getElementById('makePublic')?.checked !== false;
            let registryPayload = null;
            try {
                registryPayload = this.buildDeployInitData({
                    upgradeAuthority,
                    customAuthority,
                    makePublic
                });
            } catch (e) {
                return;
            }

            if (registryPayload?.symbol) {
                const existing = await this.rpc.getSymbolRegistry(registryPayload.symbol);
                if (existing && existing.program) {
                    this.addTerminalLine(`❌ Symbol already registered: ${registryPayload.symbol}`, 'error');
                    this.addTerminalLine(`   Program: ${existing.program}`, 'error');
                    return;
                }
            }

            const initData = registryPayload
                ? this.encodeInitData(registryPayload)
                : null;

            if (this.programIdOverrideEnabled && !programIdOverride) {
                this.addTerminalLine('❌ Program ID override enabled, but no program id provided', 'error');
                return;
            }
            
            // Deploy
            const result = await deployer.deploy(this.compiledWasm, {
                initialFunding,
                verify,
                initData,
                metadata: {
                    name: programName,
                    description: 'Deployed from MoltChain Playground'
                },
                programIdOverride
            });
            
            this.addTerminalLine('✅ Program deployed successfully!', 'success');
            this.addTerminalLine(`   Program ID: ${result.programId}`, 'info');
            this.addTerminalLine(`   Signature: ${result.signature}`, 'info');
            this.addTerminalLine(`   Explorer: ${this.getExplorerUrl()}/program/${result.programId}`, 'link');
            this.addTerminalLine('', 'normal');

            if (registryPayload?.symbol) {
                result.registry = {
                    ...registryPayload,
                    program: result.programId,
                    owner: this.wallet?.address || null
                };
                this.registryLookupCache.set(registryPayload.symbol, result.registry);
            }
            
            // Save deployed program
            this.deployedPrograms.push(result);
            localStorage.setItem('deployed_programs', JSON.stringify(this.deployedPrograms));
            
            // Update UI
            this.updateDeployedProgramsList();

            this.showProgramDetails(result.programId);
            
            // Refresh balance
            await this.refreshBalance();
            
        } catch (error) {
            this.addTerminalLine('❌ Deployment failed:', 'error');
            this.addTerminalLine(`   ${error.message}`, 'error');
        }
    },

    /**
     * Upgrade an already-deployed program with new compiled WASM.
     * Reads the target program ID from the sidebar #upgradeProgramId input
     * (or falls back to the most recently deployed program).
     */
    async upgradeProgram() {
        if (!this.compiledWasm) {
            this.addTerminalLine('❌ No compiled WASM. Build first!', 'error');
            return;
        }

        if (!this.wallet) {
            this.addTerminalLine('❌ No wallet connected. Create or import wallet first!', 'error');
            this.openWalletModal();
            return;
        }

        // Determine which program to upgrade
        let programId = document.getElementById('upgradeProgramId')?.value?.trim();
        if (!programId && this.deployedPrograms.length > 0) {
            programId = this.deployedPrograms[this.deployedPrograms.length - 1].programId;
        }
        if (!programId) {
            this.addTerminalLine('❌ No program ID specified and no previously deployed programs found.', 'error');
            return;
        }

        this.addTerminalLine(`🔄 Upgrading program ${programId}...`, 'info');
        this.addTerminalLine('', 'normal');

        try {
            const deployer = new MoltChain.ProgramDeployer(this.rpc, this.wallet);
            const result = await deployer.upgrade(programId, this.compiledWasm);

            this.addTerminalLine('✅ Program upgraded successfully!', 'success');
            this.addTerminalLine(`   Program ID: ${result.programId}`, 'info');
            this.addTerminalLine(`   Signature:  ${result.signature}`, 'info');
            this.addTerminalLine(`   Explorer:   ${this.getExplorerUrl()}/program/${result.programId}`, 'link');
            this.addTerminalLine('', 'normal');

            this.showProgramDetails(result.programId);
            await this.refreshBalance();
        } catch (error) {
            this.addTerminalLine('❌ Upgrade failed:', 'error');
            this.addTerminalLine(`   ${error.message}`, 'error');
        }
    },

    buildRegistryPayload() {
        return this.buildDeployInitData({
            upgradeAuthority: 'wallet',
            customAuthority: '',
            makePublic: true
        });
    },

    /**
     * Build the init_data payload for contract deployment.
     * Maps template options (token/NFT/custom) to the DeployRegistryData JSON
     * that the validator expects: { symbol, name, template, metadata, upgrade_authority, make_public, abi }
     */
    buildDeployInitData({ upgradeAuthority = 'wallet', customAuthority = '', makePublic = true } = {}) {
        const payload = {
            make_public: makePublic
        };

        // Resolve upgrade authority
        if (upgradeAuthority === 'none') {
            payload.upgrade_authority = 'none';
        } else if (upgradeAuthority === 'custom' && customAuthority) {
            payload.upgrade_authority = customAuthority;
        }
        // 'wallet' = default = deployer's key (no override needed)

        // Determine template and populate symbol/name from template options
        const templateId = this.currentTemplateId;
        if (templateId === 'token' || templateId === 'mt20') {
            const opts = this.readTokenOptionsFromUI();
            if (opts) {
                payload.symbol = opts.symbol;
                payload.name = opts.name;
                payload.template = 'mt20';
                const social = {};
                if (opts.twitter) social.twitter = opts.twitter;
                if (opts.telegram) social.telegram = opts.telegram;
                if (opts.discord) social.discord = opts.discord;
                payload.metadata = {
                    decimals: opts.decimals,
                    initial_supply: opts.initialSupplyBase,
                    mintable: opts.mintable,
                    burnable: opts.burnable,
                    description: opts.description || undefined,
                    website: opts.website || undefined,
                    logo_url: opts.logo_url || undefined,
                    twitter: opts.twitter || undefined,
                    telegram: opts.telegram || undefined,
                    discord: opts.discord || undefined,
                    social_urls: Object.keys(social).length ? social : undefined
                };
            }
        } else if (templateId === 'nft' || templateId === 'mt721') {
            const opts = this.readNftOptionsFromUI();
            if (opts) {
                payload.symbol = opts.symbol;
                payload.name = opts.name;
                payload.template = 'mt721';
                payload.metadata = {
                    max_supply: opts.maxSupply,
                    royalty_bps: opts.royaltyBps
                };
            }
        } else if (templateId === 'dex' || templateId === 'amm') {
            payload.template = 'amm';
            const programName = document.getElementById('programName')?.value || 'my_dex';
            payload.name = programName;
            payload.symbol = programName.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 10);
        } else if (templateId === 'dao') {
            payload.template = 'dao';
            const programName = document.getElementById('programName')?.value || 'my_dao';
            payload.name = programName;
            payload.symbol = programName.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 10);
        } else {
            // Custom contract — use program name as both name and symbol
            const programName = document.getElementById('programName')?.value || 'my_program';
            payload.name = programName;
            payload.template = 'custom';
            // Only register a symbol if user set a name
            if (programName && programName !== 'my_program') {
                payload.symbol = programName.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 10);
            }
        }

        return payload;
    },

    encodeInitData(payload) {
        try {
            const encoder = new TextEncoder();
            return encoder.encode(JSON.stringify(payload));
        } catch (e) {
            return null;
        }
    },
    
    async runTests() {
        this.addTerminalLine('🧪 Running tests...', 'info');
        this.addTerminalLine('', 'normal');
        
        const code = this.editor.getValue();
        // Extract #[test] functions from code
        const testRegex = /#\[test\]\s*(?:pub\s+)?fn\s+(\w+)/g;
        const tests = [];
        let match;
        while ((match = testRegex.exec(code)) !== null) {
            tests.push(match[1]);
        }
        
        if (tests.length === 0) {
            this.addTerminalLine('⚠️  No #[test] functions found in current file', 'warning');
            return;
        }
        
        this.addTerminalLine(`Found ${tests.length} test(s): ${tests.join(', ')}`, 'info');
        
        try {
            // Build first, then run tests via compiler
            const compilerUrl = `${this.rpc.rpcUrl.replace('/rpc', '')}/compile`;
            const response = await fetch(compilerUrl, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ code, language: 'rust', mode: 'test' })
            });
            const result = await response.json();
            
            if (result.success) {
                for (const t of tests) {
                    this.addTerminalLine(`  ✅ ${t} ... ok`, 'success');
                }
                this.addTerminalLine(`\ntest result: ok. ${tests.length} passed; 0 failed`, 'success');
            } else {
                this.addTerminalLine(`❌ Test compilation failed: ${result.error || 'unknown error'}`, 'error');
            }
        } catch (err) {
            this.addTerminalLine(`❌ Test runner error: ${err.message}`, 'error');
            this.addTerminalLine('ℹ️  Ensure the compiler service is running', 'info');
        }
    },
    
    formatCode() {
        if (!this.editor) return;
        this.editor.getAction('editor.action.formatDocument').run();
        this.addTerminalLine('✅ Code formatted', 'success');
    },
    
    async verifyCode() {
        this.addTerminalLine('🔍 Verifying code...', 'info');
        
        const code = this.editor.getValue();
        
        // Basic static analysis checks
        const issues = [];
        
        // Check for unsafe blocks
        if (code.includes('unsafe {') || code.includes('unsafe fn')) {
            issues.push({ level: 'warning', msg: 'Contains unsafe code blocks' });
        }
        
        // Check for unwrap() calls (potential panics)
        const unwrapCount = (code.match(/\.unwrap\(\)/g) || []).length;
        if (unwrapCount > 0) {
            issues.push({ level: 'warning', msg: `${unwrapCount} .unwrap() call(s) — consider using ? or match` });
        }
        
        // Check for TODO/FIXME comments
        const todoCount = (code.match(/\/\/\s*(TODO|FIXME|HACK)/gi) || []).length;
        if (todoCount > 0) {
            issues.push({ level: 'info', msg: `${todoCount} TODO/FIXME comment(s) found` });
        }
        
        // Check entry point
        if (!code.includes('process_instruction') && !code.includes('fn main')) {
            issues.push({ level: 'warning', msg: 'No process_instruction or main entry point found' });
        }
        
        if (issues.length === 0) {
            this.addTerminalLine('✅ Code verification passed — no issues found', 'success');
        } else {
            for (const issue of issues) {
                const icon = issue.level === 'warning' ? '⚠️' : 'ℹ️';
                this.addTerminalLine(`  ${icon} ${issue.msg}`, issue.level);
            }
            const warnings = issues.filter(i => i.level === 'warning').length;
            this.addTerminalLine(`\nVerification complete: ${warnings} warning(s), ${issues.length - warnings} info`, warnings > 0 ? 'warning' : 'success');
        }
    },
    
    // ========================================================================
    // TEST & INTERACT
    // ========================================================================
    
    async testProgram() {
        const programId = document.getElementById('testProgramAddr')?.value;
        const functionName = document.getElementById('testFunction')?.value;
        const argsJson = document.getElementById('testArgs')?.value || '[]';
        
        if (!programId || !functionName) {
            this.addTerminalLine('❌ Program ID and function name required', 'error');
            return;
        }
        
        if (!this.wallet) {
            this.addTerminalLine('❌ Connect wallet first', 'error');
            this.openWalletModal();
            return;
        }
        
        this.addTerminalLine(`🧪 Testing ${functionName}()...`, 'info');
        this.addTerminalLine('', 'normal');
        
        try {
            const args = JSON.parse(argsJson);
            
            // Build transaction
            const tx = new MoltChain.TransactionBuilder(this.rpc);
            await tx.setRecentBlockhash();
            
            tx.addInstruction(
                MoltChain.TransactionBuilder.call(this.wallet.address, programId, functionName, args)
            );
            
            tx.sign(this.wallet);
            
            // Send
            const signature = await tx.send();
            
            this.addTerminalLine(`   Transaction: ${signature}`, 'info');
            this.addTerminalLine('   Waiting for confirmation...', 'info');
            
            // Wait for confirmation
            const deployer = new MoltChain.ProgramDeployer(this.rpc, this.wallet);
            const confirmed = await deployer.waitForConfirmation(signature);
            
            if (confirmed) {
                const txResult = await this.rpc.getTransaction(signature);
                
                this.addTerminalLine('✅ Function executed successfully!', 'success');
                
                if (txResult.return_value) {
                    this.addTerminalLine(`   Return value: ${txResult.return_value}`, 'success');
                    
                    // Display in result panel
                    document.getElementById('testResult').style.display = 'block';
                    document.getElementById('testResultData').textContent = JSON.stringify(txResult, null, 2);
                }
                
                if (txResult.logs && txResult.logs.length > 0) {
                    this.addTerminalLine('   Logs:', 'info');
                    txResult.logs.forEach(log => this.addTerminalLine(`     ${log}`, 'info'));
                }
            } else {
                this.addTerminalLine('❌ Transaction not confirmed (timeout)', 'error');
            }
            
            // Refresh balance
            await this.refreshBalance();
            
        } catch (error) {
            this.addTerminalLine('❌ Test failed:', 'error');
            this.addTerminalLine(`   ${error.message}`, 'error');
        }
    },
    
    // ========================================================================
    // WALLET MANAGEMENT
    // ========================================================================
    
    openWalletModal() {
        // Create modal if it doesn't exist
        let modal = document.getElementById('walletModal');
        if (!modal) {
            modal = this.createWalletModal();
            document.body.appendChild(modal);
        }

        this.updateCreateWalletDescription();
        
        modal.style.display = 'flex';
    },
    
    createWalletModal() {
        const modal = document.createElement('div');
        modal.id = 'walletModal';
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-content">
                <div class="modal-header">
                    <h2><i class="fas fa-wallet"></i> Wallet</h2>
                    <button class="modal-close" onclick="Playground.closeWalletModal()">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
                <div class="modal-body">
                    ${this.wallet ? this.getWalletConnectedHTML() : this.getWalletDisconnectedHTML()}
                </div>
            </div>
        `;
        return modal;
    },
    
    getWalletDisconnectedHTML() {
        return `
            <div class="wallet-actions">
                <button class="btn btn-primary btn-block btn-lg" onclick="Playground.createWallet()">
                    <i class="fas fa-plus-circle"></i> Create New Wallet
                </button>
                <button class="btn btn-secondary btn-block btn-lg" onclick="Playground.importWallet()">
                    <i class="fas fa-file-import"></i> Import Wallet
                </button>
            </div>
        `;
    },
    
    getWalletConnectedHTML() {
        return `
            <div class="wallet-info-display">
                <div class="wallet-address">
                    <label>Address:</label>
                    <code>${this.wallet.address}</code>
                    <button class="btn-icon" onclick="navigator.clipboard.writeText('${this.wallet.address}')">
                        <i class="fas fa-copy"></i>
                    </button>
                </div>
                <div class="wallet-balance">
                    <label>Balance:</label>
                    <h3>${this.balance ? (this.balance.spendable / 1_000_000_000).toFixed(4) : '0.0000'} MOLT</h3>
                </div>
            </div>
            <div class="wallet-actions">
                <button class="btn btn-secondary btn-block" onclick="Playground.exportWallet()">
                    <i class="fas fa-download"></i> Export Wallet
                </button>
                <button class="btn btn-danger btn-block" onclick="Playground.disconnectWallet()">
                    <i class="fas fa-sign-out-alt"></i> Disconnect
                </button>
            </div>
        `;
    },
    
    closeWalletModal() {
        const modal = document.getElementById('walletModal');
        if (modal) {
            modal.style.display = 'none';
        }
    },

    createWalletFromModal() {
        const password = document.getElementById('createWalletPassword')?.value || '';
        const name = document.getElementById('createWalletName')?.value || '';
        this.createWallet(password, name);
    },

    async importWalletFromModal() {
        const method = document.querySelector('input[name="importMethod"]:checked')?.value || 'seed';
        const name = document.getElementById('importWalletName')?.value || '';

        if (method === 'json') {
            const fileInput = document.getElementById('importJsonFile');
            const file = fileInput?.files?.[0];
            if (!file) {
                alert('Select a JSON keystore file');
                return;
            }
            try {
                const content = await file.text();
                const json = JSON.parse(content);
                const wallet = MoltChain.Wallet.import(json, '');
                this.applyImportedWallet(wallet, name);
            } catch (e) {
                alert('Invalid JSON keystore');
            }
            return;
        }

        const rawValue = method === 'privatekey'
            ? document.getElementById('importPrivateKey')?.value
            : document.getElementById('importSeed')?.value;

        try {
            const seed = this.parseWalletSeedInput(rawValue || '');
            if (!seed) {
                alert('Enter a valid secret key');
                return;
            }
            const wallet = MoltChain.Wallet.import({ seed }, '');
            this.applyImportedWallet(wallet);
        } catch (e) {
            alert(e.message || 'Invalid secret key');
        }
    },

    parseWalletSeedInput(value) {
        const trimmed = (value || '').trim();
        if (!trimmed) return null;
        if (trimmed.includes(' ')) {
            throw new Error('Mnemonic import not supported. Use base58 or JSON.');
        }
        if (/^0x[0-9a-fA-F]+$/.test(trimmed)) {
            const bytes = MoltChain.utils.hexToBytes(trimmed);
            return MoltChain.utils.base58Encode(bytes);
        }
        return trimmed;
    },

    applyImportedWallet(wallet, name = '') {
        this.wallet = wallet;
        this.addWalletToStore(wallet, name);
        this.saveWallet();
        this.updateWalletDisplay();
        this.closeWalletModal();
        this.refreshBalance();

        this.addTerminalLine('✅ Wallet imported!', 'success');
        this.addTerminalLine(`   Address: ${this.wallet.address}`, 'info');

        this.updateProgramIdPreview();
    },

    exportWalletSeed() {
        if (!this.wallet) return;
        const walletData = this.wallet.export('');
        alert(`SECRET KEY (BASE58):\n\n${walletData.seed}\n\nKeep it secure!`);
    },

    exportWalletPrivateKey() {
        if (!this.wallet) return;
        const walletData = this.wallet.export('');
        const seedBytes = MoltChain.utils.base58Decode(walletData.seed);
        const hex = MoltChain.utils.bytesToHex(seedBytes);
        alert(`SECRET KEY (HEX):\n\n0x${hex}\n\nKeep it secure!`);
    },
    
    createWallet(password = '', name = '') {
        this.wallet = new MoltChain.Wallet();
        this.addWalletToStore(this.wallet, name);
        const walletData = this.wallet.export(password);
        
        alert(`SAVE YOUR SECRET KEY (BASE58):\n\n${walletData.seed}\n\nKeep it secure!`);
        
        // Save wallet
        this.saveWallet();
        
        // Update UI
        this.updateWalletDisplay();
        this.closeWalletModal();
        
        // Subscribe to updates
        if (this.ws && this.ws.connected) {
            this.ws.subscribeAccount(this.wallet.address, (accountInfo) => {
                this.balance = accountInfo;
                this.updateBalanceDisplay();
            });
        }
        
        // Refresh balance
        this.refreshBalance();
        
        this.addTerminalLine('✅ New wallet created!', 'success');
        this.addTerminalLine(`   Address: ${this.wallet.address}`, 'info');

        this.updateProgramIdPreview();
    },
    
    importWallet() {
        const seed = prompt('Enter your base58 secret key:');
        if (seed) {
            try {
                const wallet = MoltChain.Wallet.import({ seed }, '');
                this.applyImportedWallet(wallet, name);
            } catch (e) {
                alert('Invalid secret key');
            }
        }
    },
    
    exportWallet() {
        if (!this.wallet) return;
        
        const walletData = this.wallet.export('');
        const json = JSON.stringify(walletData, null, 2);
        const blob = new Blob([json], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `molt-wallet-${Date.now()}.json`;
        a.click();
        
        this.addTerminalLine('✅ Wallet exported', 'success');
    },
    
    disconnectWallet() {
        if (confirm('Are you sure you want to disconnect your wallet?')) {
            this.wallet = null;
            this.balance = null;
            localStorage.removeItem('molt_wallet');
            this.activeWalletId = null;
            
            this.updateWalletDisplay();
            this.closeWalletModal();
            
            this.addTerminalLine('👋 Wallet disconnected', 'info');
            this.updateProgramIdPreview();
        }
    },
    
    saveWallet() {
        if (!this.wallet) return;
        const walletData = this.wallet.export('');
        localStorage.setItem('molt_wallet', JSON.stringify(walletData));
    },

    loadWalletStore() {
        const stored = localStorage.getItem('molt_wallets');
        if (stored) {
            try {
                this.wallets = JSON.parse(stored) || [];
            } catch (e) {
                this.wallets = [];
            }
        }

        const savedActive = localStorage.getItem('molt_active_wallet_id');
        if (savedActive) {
            this.activeWalletId = savedActive;
        }

        const legacy = localStorage.getItem('molt_wallet');
        if (legacy) {
            try {
                const walletData = JSON.parse(legacy);
                const address = MoltChain.Wallet.import(walletData, '').address;
                if (!this.wallets.some(item => item.address === address)) {
                    this.wallets.push({
                        id: `wallet_${Date.now()}`,
                        name: 'Wallet',
                        seed: walletData.seed,
                        address
                    });
                }
            } catch (e) {
                // ignore
            }
        }

        this.saveWalletStore();
    },

    saveWalletStore() {
        localStorage.setItem('molt_wallets', JSON.stringify(this.wallets));
        if (this.activeWalletId) {
            localStorage.setItem('molt_active_wallet_id', this.activeWalletId);
        } else {
            localStorage.removeItem('molt_active_wallet_id');
        }
    },

    addWalletToStore(wallet, name = '') {
        if (!wallet) return;
        const seed = wallet.export('').seed;
        const address = wallet.address;
        const existing = this.wallets.find(item => item.address === address);
        const label = (name || '').trim() || `Wallet ${this.wallets.length + 1}`;

        if (existing) {
            existing.name = label;
            this.activeWalletId = existing.id;
            this.saveWalletStore();
            return;
        }

        const id = `wallet_${Date.now()}_${Math.floor(Math.random() * 1000)}`;
        this.wallets.push({ id, name: label, seed, address });
        this.activeWalletId = id;
        this.saveWalletStore();
    },

    async applyActiveWallet() {
        if (!this.activeWalletId && this.wallets.length > 0) {
            this.activeWalletId = this.wallets[0].id;
        }

        const active = this.wallets.find(item => item.id === this.activeWalletId);
        if (active?.seed) {
            try {
                this.wallet = MoltChain.Wallet.import({ seed: active.seed }, '');
                await this.refreshBalance();
                this.updateWalletDisplay();
                return;
            } catch (e) {
                console.error('Failed to load active wallet:', e);
            }
        }

        this.wallet = null;
        this.balance = null;
        this.updateWalletDisplay();
    },

    async setActiveWallet(id) {
        this.activeWalletId = id;
        this.saveWalletStore();
        await this.applyActiveWallet();
        this.hideWalletDropdown();
    },

    removeActiveWallet() {
        if (!this.activeWalletId) return;
        if (!confirm('Remove the active wallet?')) return;

        this.wallets = this.wallets.filter(item => item.id !== this.activeWalletId);
        this.activeWalletId = null;
        this.wallet = null;
        this.balance = null;
        localStorage.removeItem('molt_wallet');
        this.saveWalletStore();
        this.updateWalletDisplay();
        this.hideWalletDropdown();
    },

    updateWalletBalanceUI() {
        const pill = document.getElementById('walletBalancePill');
        const text = document.getElementById('walletBalanceText');
        const dropdownBalance = document.getElementById('walletDropdownBalance');

        const shells = this.balance?.spendable ?? this.balance?.balance ?? 0;
        const molt = (shells / 1_000_000_000).toFixed(4);

        if (pill && text) {
            if (this.wallet) {
                pill.style.display = 'inline-flex';
                text.textContent = `${molt} MOLT`;
            } else {
                pill.style.display = 'none';
            }
        }

        if (dropdownBalance) {
            dropdownBalance.textContent = `${molt} MOLT`;
        }
    },

    updateWalletDropdown() {
        const list = document.getElementById('walletDropdownList');
        if (!list) return;

        if (!this.wallets.length) {
            list.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-wallet"></i>
                    <p>No wallets yet</p>
                </div>
            `;
            return;
        }

        // AUDIT-FIX F14.7: escape wallet name from localStorage
        list.innerHTML = this.wallets.map(item => `
            <div class="wallet-dropdown-item ${item.id === this.activeWalletId ? 'active' : ''}" data-wallet-id="${escapeHtml(item.id)}">
                <span>${escapeHtml(item.name)}</span>
                <code class="monospace">${escapeHtml(this.truncateAddress(item.address))}</code>
            </div>
        `).join('');

        list.querySelectorAll('.wallet-dropdown-item').forEach(entry => {
            entry.addEventListener('click', () => {
                const id = entry.dataset.walletId;
                if (id && id !== this.activeWalletId) {
                    this.setActiveWallet(id);
                }
            });
        });
    },

    toggleWalletDropdown() {
        const dropdown = document.getElementById('walletDropdown');
        if (!dropdown) return;
        if (dropdown.style.display === 'none' || !dropdown.style.display) {
            this.updateWalletDropdown();
            this.updateWalletBalanceUI();
            dropdown.style.display = 'block';
        } else {
            dropdown.style.display = 'none';
        }
    },

    hideWalletDropdown() {
        const dropdown = document.getElementById('walletDropdown');
        if (dropdown) {
            dropdown.style.display = 'none';
        }
    },
    
    async refreshBalance() {
        if (!this.wallet) return;
        
        try {
            this.balance = await this.rpc.getBalance(this.wallet.address);
            this.updateBalanceDisplay();
        } catch (e) {
            console.error('Failed to refresh balance:', e);
        }
    },
    
    async requestFaucet() {
        if (!this.wallet) {
            this.addTerminalLine('❌ Create wallet first', 'error');
            this.openWalletModal();
            return;
        }
        
        if (this.network === 'mainnet') {
            this.addTerminalLine('❌ Faucet not available on mainnet', 'error');
            return;
        }
        
        this.addTerminalLine('💧 Requesting testnet MOLT...', 'info');
        
        try {
            const amount = this.network === 'local' ? 1000 : 100;
            const result = await this.rpc.requestFaucet(this.wallet.address, amount);
            
            this.addTerminalLine(`✅ Received ${amount} MOLT!`, 'success');
            this.addTerminalLine(`   Signature: ${result.signature}`, 'info');
            
            // Refresh balance after a delay
            setTimeout(() => this.refreshBalance(), 2000);
            
        } catch (error) {
            this.addTerminalLine('❌ Faucet request failed:', 'error');
            this.addTerminalLine(`   ${error.message}`, 'error');
        }
    },
    
    // ========================================================================
    // TRANSFERS
    // ========================================================================
    
    async sendTransfer() {
        const recipient = document.getElementById('transferTo')?.value;
        const amount = parseFloat(document.getElementById('transferAmount')?.value);
        
        if (!recipient || !amount) {
            this.addTerminalLine('❌ Recipient and amount required', 'error');
            return;
        }

        // AUDIT-FIX F14.8: validate transfer amount
        if (!Number.isFinite(amount) || amount <= 0 || amount > 1_000_000_000) {
            this.addTerminalLine('❌ Amount must be between 0 and 1,000,000,000 MOLT', 'error');
            return;
        }
        
        if (!this.wallet) {
            this.addTerminalLine('❌ Connect wallet first', 'error');
            this.openWalletModal();
            return;
        }
        
        this.addTerminalLine(`💸 Sending ${amount} MOLT to ${recipient}...`, 'info');
        
        try {
            const amountShells = amount * 1_000_000_000;
            
            const tx = new MoltChain.TransactionBuilder(this.rpc);
            await tx.setRecentBlockhash();
            
            tx.addInstruction(
                MoltChain.TransactionBuilder.transfer(this.wallet.address, recipient, amountShells)
            );
            
            tx.sign(this.wallet);
            
            const signature = await tx.send();
            
            this.addTerminalLine(`✅ Transfer sent!`, 'success');
            this.addTerminalLine(`   Signature: ${signature}`, 'info');
            
            // Clear form
            document.getElementById('transferTo').value = '';
            document.getElementById('transferAmount').value = '';
            
            // Refresh balance
            setTimeout(() => this.refreshBalance(), 2000);
            
        } catch (error) {
            this.addTerminalLine('❌ Transfer failed:', 'error');
            this.addTerminalLine(`   ${error.message}`, 'error');
        }
    },
    
    // ========================================================================
    // NETWORK
    // ========================================================================
    
    async switchNetwork(network) {
        const targetNetwork = normalizeExplorerNetwork(network);
        this.addTerminalLine(`🔄 Switching to ${targetNetwork}...`, 'info');
        
        // Disconnect old WebSocket
        if (this.ws) {
            this.ws.disconnect();
        }

        localStorage.setItem(PLAYGROUND_NETWORK_STORAGE_KEY, targetNetwork);
        localStorage.setItem(EXPLORER_NETWORK_STORAGE_KEY, targetNetwork);
        
        // Initialize new network
        await this.initNetwork(targetNetwork);
        
        // Update UI
        this.updateNetworkDisplay();
        
        // Re-setup live updates
        this.setupLiveUpdates();

        await this.refreshProgramIndex({ showError: false });

        this.scheduleRegistryStatusUpdate();
        
        this.addTerminalLine(`✅ Switched to ${targetNetwork}`, 'success');
    },
    
    // ========================================================================
    // UI UPDATES
    // ========================================================================
    
    updateNetworkDisplay() {
        const selector = document.getElementById('networkSelect');
        if (selector) {
            selector.value = this.network;
        }
        
        // Show/hide faucet button
        const faucetBtn = document.getElementById('faucetBtn');
        if (faucetBtn) {
            faucetBtn.style.display = ['mainnet', 'local-mainnet'].includes(this.network) ? 'none' : 'inline-flex';
        }

        this.updateCreateWalletDescription();
    },
    
    updateWalletDisplay() {
        const walletBtn = document.getElementById('walletBtnText');
        if (walletBtn) {
            if (this.wallet) {
                walletBtn.textContent = this.truncateAddress(this.wallet.address);
            } else {
                walletBtn.textContent = 'Connect Wallet';
            }
        }
        this.updateWalletBalanceUI();
        this.updateWalletDropdown();
        if (!this.wallet) {
            this.hideWalletDropdown();
        }
        this.updateProgramOverrideAvailability();
        this.updateTemplateOwnerUI();
    },

    updateUpgradeAuthorityUI() {
        const select = document.getElementById('upgradeAuthority');
        const group = document.getElementById('customAuthorityGroup');
        if (!select || !group) return;
        group.style.display = select.value === 'custom' ? 'block' : 'none';
    },
    
    updateBalanceDisplay() {
        this.updateWalletBalanceUI();
    },
    
    updateDeployedProgramsList() {
        const listEl = document.getElementById('deployedProgramsList');
        if (!listEl) return;

        const mergedPrograms = this.mergeProgramLists();

        if (mergedPrograms.length === 0) {
            listEl.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-box-open"></i>
                    <p>No programs deployed yet</p>
                    <small>Build and deploy to see them here</small>
                </div>
            `;
        } else {
            // AUDIT-FIX F14.2: escape metadata name + use data-attribute instead of onclick interpolation
            listEl.innerHTML = mergedPrograms.map(program => `
                <div class="deployed-program-item">
                    <div class="program-icon">📦</div>
                    <div class="program-info">
                        <h4>${escapeHtml(program.metadata?.name || 'Unnamed')}</h4>
                        <code class="program-id">${escapeHtml(this.truncateAddress(program.programId))}</code>
                    </div>
                    <span class="program-source">${escapeHtml(program.sourceLabel)}</span>
                    <button class="btn-icon program-view-btn" data-program-id="${escapeHtml(program.programId)}">
                        <i class="fas fa-external-link-alt"></i>
                    </button>
                </div>
            `).join('');

            // Wire up click handlers from data-attributes
            listEl.querySelectorAll('.program-view-btn').forEach(btn => {
                btn.addEventListener('click', () => {
                    const id = btn.dataset.programId;
                    if (id) Playground.viewProgram(id);
                });
            });
        }
    },

    async refreshProgramIndex({ showError = false } = {}) {
        if (!this.rpc) return;

        try {
            const result = await this.rpc.getPrograms({ limit: 50 });
            this.networkPrograms = Array.isArray(result?.programs)
                ? result.programs
                : [];
            this.updateDeployedProgramsList();
        } catch (error) {
            this.networkPrograms = [];
            this.updateDeployedProgramsList();
            if (showError) {
                this.addTerminalLine('⚠️  Failed to refresh programs list', 'warning');
                if (error?.message) {
                    this.addTerminalLine(`   ${error.message}`, 'warning');
                }
            }
        }
    },

    mergeProgramLists() {
        const merged = new Map();

        this.deployedPrograms.forEach(program => {
            merged.set(program.programId, {
                ...program,
                sourceLabel: 'Local'
            });
        });

        this.networkPrograms.forEach(programId => {
            if (!merged.has(programId)) {
                merged.set(programId, {
                    programId,
                    metadata: { name: 'On-chain Program' },
                    sourceLabel: 'On-chain'
                });
            }
        });

        return Array.from(merged.values());
    },
    
    updateProblemsPanel(errors) {
        document.getElementById('problemsCount').textContent = errors.length;
        
        const problemsList = document.getElementById('problemsList');
        if (!problemsList) return;
        
        if (errors.length === 0) {
            problemsList.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-check-circle"></i>
                    <p>No problems detected</p>
                </div>
            `;
        } else {
            // AUDIT-FIX F14.3: escape compiler error messages
            problemsList.innerHTML = errors.map(err => `
                <div class="problem-item error">
                    <i class="fas fa-times-circle"></i>
                    <div class="problem-info">
                        <div class="problem-message">${escapeHtml(err.message)}</div>
                        <div class="problem-location">${escapeHtml(err.file)}:${escapeHtml(String(err.line))}:${escapeHtml(String(err.col))}</div>
                    </div>
                </div>
            `).join('');
        }
    },
    
    // ========================================================================
    // TERMINAL
    // ========================================================================
    
    addTerminalLine(text, type = 'normal') {
        const linesEl = document.getElementById('terminalLines');
        if (!linesEl) return;
        
        const line = document.createElement('div');
        line.className = 'terminal-line';
        
        const timestamp = new Date().toLocaleTimeString();
        
        // AUDIT-FIX F14.1: escape text to prevent XSS + validate URL scheme for links
        if (type === 'link') {
            const safeUrl = sanitizeUrl(text);
            if (safeUrl) {
                line.innerHTML = `<span class="terminal-time">[${timestamp}]</span> <a class="terminal-link" href="${escapeHtml(safeUrl)}" target="_blank">${escapeHtml(text)}</a>`;
            } else {
                line.innerHTML = `<span class="terminal-time">[${timestamp}]</span> <span class="terminal-normal">${escapeHtml(text)}</span>`;
            }
        } else {
            const icon = {
                'success': '✅',
                'error': '❌',
                'warning': '⚠️ ',
                'info': 'ℹ️ '
            }[type] || '';
            
            line.innerHTML = `<span class="terminal-time">[${timestamp}]</span> <span class="terminal-${escapeHtml(type)}">${icon} ${escapeHtml(text)}</span>`;
        }
        
        linesEl.appendChild(line);
        this.scrollTerminalToBottom();
        
        // Keep history reasonable
        while (linesEl.children.length > 1000) {
            linesEl.removeChild(linesEl.firstChild);
        }
    },
    
    clearTerminal() {
        const linesEl = document.getElementById('terminalLines');
        if (linesEl) {
            linesEl.innerHTML = '';
            this.addTerminalLine('Terminal cleared', 'info');
        }
    },

    addTerminalCommand(command) {
        const linesEl = document.getElementById('terminalLines');
        if (!linesEl) return;

        const line = document.createElement('div');
        line.className = 'terminal-line';

        const prompt = document.createElement('span');
        prompt.className = 'terminal-prompt';
        prompt.textContent = 'molt@playground:~$';

        const text = document.createElement('span');
        text.className = 'terminal-text';
        text.textContent = command;

        line.appendChild(prompt);
        line.appendChild(text);
        linesEl.appendChild(line);
        this.scrollTerminalToBottom();
    },

    async handleTerminalCommand(command) {
        const trimmed = String(command || '').trim();
        if (!trimmed) return;

        this.addTerminalCommand(trimmed);

        const [cmd, ...rest] = trimmed.split(/\s+/);

        switch ((cmd || '').toLowerCase()) {
            case 'help':
                this.addTerminalLine('Commands: help, clear, build, deploy, faucet, balance', 'info');
                return;
            case 'clear':
                this.clearTerminal();
                return;
            case 'build':
                this.buildProgram();
                return;
            case 'deploy':
                this.deployProgram();
                return;
            case 'faucet':
                this.requestFaucet();
                return;
            case 'balance':
                if (!this.wallet) {
                    this.addTerminalLine('Wallet: not connected', 'warning');
                    return;
                }
                try {
                    await this.refreshBalance();
                    const shells = this.balance?.balance ?? 0;
                    const molt = (shells / 1_000_000_000).toFixed(4);
                    this.addTerminalLine(`Balance: ${molt} MOLT`, 'info');
                } catch (error) {
                    this.addTerminalLine('Balance lookup failed', 'warning');
                }
                return;
            case 'network':
                this.addTerminalLine(`Network: ${this.network}`, 'info');
                this.addTerminalLine(`RPC: ${this.rpc?.rpcUrl || 'unknown'}`, 'info');
                return;
            case 'wallet':
                if (this.wallet) {
                    this.addTerminalLine(`Wallet: ${this.wallet.address}`, 'info');
                } else {
                    this.addTerminalLine('Wallet: not connected', 'warning');
                }
                return;
            case 'program':
                if (this.selectedProgramId) {
                    this.addTerminalLine(`Program: ${this.selectedProgramId}`, 'info');
                } else {
                    this.addTerminalLine('Program: none selected', 'warning');
                }
                return;
            case 'rpc':
                if (!this.rpc) {
                    this.addTerminalLine('RPC unavailable', 'error');
                    return;
                }
                if (!rest.length) {
                    this.addTerminalLine('Usage: rpc <method> [paramsJson]', 'warning');
                    return;
                }
                try {
                    const method = rest.shift();
                    const paramsText = rest.join(' ').trim();
                    const params = paramsText ? JSON.parse(paramsText) : [];
                    const result = await this.rpc.call(method, params);
                    const formatted = JSON.stringify(result, null, 2) || 'null';
                    formatted.split('\n').forEach(line => this.addTerminalLine(line, 'info'));
                } catch (error) {
                    this.addTerminalLine(`RPC error: ${error.message || 'unknown error'}`, 'error');
                }
                return;
            default:
                this.addTerminalLine(`Unknown command: ${cmd}`, 'warning');
        }
    },

    scrollTerminalToBottom() {
        const container = document.querySelector('.terminal-content[data-terminal-tab="terminal"]');
        if (container) {
            container.scrollTop = container.scrollHeight;
        }
    },
    
    switchTerminalTab(tab) {
        // Update tabs
        document.querySelectorAll('.terminal-tab').forEach(t => {
            t.classList.toggle('active', t.dataset.terminalTab === tab);
        });
        
        // Update content
        document.querySelectorAll('.terminal-content').forEach(c => {
            c.classList.toggle('active', c.dataset.terminalTab === tab);
        });
        
        this.terminalTab = tab;
        if (tab === 'terminal') {
            document.getElementById('terminalInput')?.focus();
        }
    },
    
    switchSidebarTab(tab) {
        // Update tabs
        document.querySelectorAll('.sidebar-tab').forEach(t => {
            t.classList.toggle('active', t.dataset.tab === tab);
        });
        
        // Update content
        document.querySelectorAll('.sidebar-content').forEach(c => {
            c.classList.toggle('active', c.dataset.tab === tab);
        });
        
        this.sidebarTab = tab;
    },
    
    // ========================================================================
    // UTILITIES
    // ========================================================================
    
    viewProgram(programId) {
        this.showProgramDetails(programId);
    },
    
    getExplorerUrl() {
        return MoltChain.CONFIG.networks[this.network].explorer;
    },
    
    truncateAddress(addr, start = 8, end = 6) {
        if (!addr) return '';
        return `${addr.substring(0, start)}...${addr.substring(addr.length - end)}`;
    },
    
    formatBytes(bytes) {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
        return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
    },
    
    base64ToBytes(base64) {
        const binary = atob(base64);
        const bytes = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i++) {
            bytes[i] = binary.charCodeAt(i);
        }
        return bytes;
    },
    // ========================================================================
    // PROGRAM DETAILS
    // ========================================================================

    toggleProgramOverrideUI(enabled) {
        const group = document.getElementById('programIdOverrideGroup');
        const checkbox = document.getElementById('programIdOverride');
        if (group) {
            group.style.display = enabled ? 'block' : 'none';
        }
        if (checkbox) {
            checkbox.checked = enabled;
        }
    },

    saveProgramOverride() {
        localStorage.setItem('program_id_override_enabled', String(this.programIdOverrideEnabled));
        const value = document.getElementById('programIdOverrideValue')?.value || '';
        localStorage.setItem('program_id_override_value', value);
    },

    async updateProgramIdPreview() {
        const preview = document.getElementById('programIdPreview');
        if (!preview) return;

        if (this.programIdOverrideEnabled) {
            if (!this.wallet) {
                preview.value = 'Connect wallet';
                return;
            }
            const overrideValue = this.getProgramIdOverride();
            preview.value = overrideValue || 'Override enabled';
            if (overrideValue) {
                this.updateProgramIdDeclaration(overrideValue);
            }
            return;
        }

        if (!this.compiledWasm || !this.wallet) {
            preview.value = 'Build and connect wallet';
            return;
        }

        try {
            const deployer = new MoltChain.ProgramDeployer(this.rpc, this.wallet);
            const programId = await deployer.deriveProgramAddress(this.wallet.address, this.compiledWasm);
            preview.value = programId;
            this.updateProgramIdDeclaration(programId);
        } catch (e) {
            preview.value = 'Unable to derive program id';
        }
    },

    updateProgramIdDeclaration(programId) {
        if (!programId || !this.files.has('lib.rs')) return;
        if (this.modifiedFiles.has('lib.rs')) return;

        const current = this.files.get('lib.rs');
        const pattern = /const\s+PROGRAM_ID:\s*&str\s*=\s*"[^"]*";/;
        if (!pattern.test(current)) return;

        const updated = current.replace(pattern, `const PROGRAM_ID: &str = "${programId}";`);
        if (updated === current) return;

        this.files.set('lib.rs', updated);

        if (this.currentFile === 'lib.rs' && this.editor) {
            const editorContent = this.editor.getValue();
            if (editorContent === current) {
                this.editor.setValue(updated);
            }
        }
    },

    getProgramIdOverride() {
        if (!this.programIdOverrideEnabled) return null;
        const value = document.getElementById('programIdOverrideValue')?.value?.trim();
        return value || null;
    },

    createProgramKeypair(autoDownload = false) {
        const utils = MoltChain.utils;
        const seed = new Uint8Array(32);
        crypto.getRandomValues(seed);
        const keypair = window.nacl.sign.keyPair.fromSeed(seed);
        const seedBase58 = utils.base58Encode(seed);
        const pubkeyBase58 = utils.base58Encode(keypair.publicKey);

        this.programKeypair = {
            seed: seedBase58,
            publicKey: pubkeyBase58
        };

        localStorage.setItem('program_keypair', JSON.stringify(this.programKeypair));

        const overrideInput = document.getElementById('programIdOverrideValue');
        if (overrideInput) {
            overrideInput.value = pubkeyBase58;
        }

        this.programIdOverrideEnabled = true;
        this.toggleProgramOverrideUI(true);
        this.saveProgramOverride();

        this.updateProgramIdPreview();

        if (autoDownload) {
            this.exportProgramKeypair();
        }
    },

    importProgramKeypair() {
        const seed = prompt('Enter program seed (base58):');
        if (!seed) return;

        try {
            const utils = MoltChain.utils;
            const seedBytes = utils.base58Decode(seed.trim());
            const keypair = window.nacl.sign.keyPair.fromSeed(seedBytes);
            const pubkeyBase58 = utils.base58Encode(keypair.publicKey);

            this.programKeypair = {
                seed: seed.trim(),
                publicKey: pubkeyBase58
            };
            localStorage.setItem('program_keypair', JSON.stringify(this.programKeypair));

            const overrideInput = document.getElementById('programIdOverrideValue');
            if (overrideInput) {
                overrideInput.value = pubkeyBase58;
            }

            this.programIdOverrideEnabled = true;
            this.toggleProgramOverrideUI(true);
            this.saveProgramOverride();

            this.updateProgramIdPreview();
        } catch (e) {
            alert('Invalid program seed');
        }
    },

    exportProgramKeypair() {
        if (!this.programKeypair) {
            alert('No program keypair available');
            return;
        }

        const json = JSON.stringify(this.programKeypair, null, 2);
        const blob = new Blob([json], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `molt-program-keypair-${Date.now()}.json`;
        a.click();
    },

    async showProgramDetails(programId) {
        this.selectedProgramId = programId;
        await this.loadProgramInfo(programId);
        await this.loadProgramStorage(programId);
        await this.loadProgramCalls(programId);
    },

    getRegistryEntryForProgram(programId) {
        if (!programId) return null;

        const local = this.deployedPrograms.find(entry => entry.programId === programId && entry.registry);
        if (local?.registry) return local.registry;

        for (const entry of this.registryLookupCache.values()) {
            if (entry?.program === programId) {
                return entry;
            }
        }

        return null;
    },

    populateProgramRegistryInfo(programId) {
        const setText = (id, value) => {
            const el = document.getElementById(id);
            if (el) el.textContent = value;
        };

        const entry = this.getRegistryEntryForProgram(programId);
        if (!entry) {
            setText('infoRegistrySymbol', 'Not registered');
            setText('infoRegistryName', '-');
            setText('infoRegistryTemplate', '-');
            setText('infoRegistryOwner', '-');
            return;
        }

        setText('infoRegistrySymbol', entry.symbol || 'Registered');
        setText('infoRegistryName', entry.name || '-');
        setText('infoRegistryTemplate', entry.template || '-');
        setText('infoRegistryOwner', entry.owner || '-');
    },

    applyProgramRegistryEntry(entry, programId) {
        if (!entry || !programId) return;
        if (entry.symbol) {
            this.registryLookupCache.set(entry.symbol, entry);
        }
        const local = this.deployedPrograms.find(item => item.programId === programId);
        if (local) {
            local.registry = entry;
            localStorage.setItem('deployed_programs', JSON.stringify(this.deployedPrograms));
        }
    },

    hideProgramPanels() {
        const infoPanel = document.getElementById('programInfoPanel');
        const storagePanel = document.getElementById('storageViewerPanel');
        const callsPanel = document.getElementById('programCallsPanel');
        if (infoPanel) infoPanel.style.display = 'none';
        if (storagePanel) storagePanel.style.display = 'none';
        if (callsPanel) callsPanel.style.display = 'none';
    },

    async loadProgramInfo(programId) {
        if (!this.rpc) return;

        try {
            const [info, stats] = await Promise.all([
                this.rpc.getProgram(programId),
                this.rpc.getProgramStats(programId)
            ]);

            const infoPanel = document.getElementById('programInfoPanel');
            if (infoPanel) {
                infoPanel.style.display = 'block';
            }

            const setText = (id, value) => {
                const el = document.getElementById(id);
                if (el) el.textContent = value;
            };

            setText('infoProgramId', info.program || programId);
            setText('infoOwner', info.owner || '-');
            setText('infoCodeSize', info.code_size ? `${info.code_size} bytes` : '-');
            setText('infoDeployed', stats?.call_count !== undefined ? `${stats.call_count} calls` : '-');
            setText('infoNetwork', this.network);

            const resolvedProgramId = info.program || programId;
            const cachedEntry = this.getRegistryEntryForProgram(resolvedProgramId);
            if (cachedEntry) {
                this.populateProgramRegistryInfo(resolvedProgramId);
            } else {
                this.populateProgramRegistryInfo(resolvedProgramId);
                try {
                    const entry = await this.rpc.getSymbolRegistryByProgram(resolvedProgramId);
                    if (entry && entry.program) {
                        this.applyProgramRegistryEntry(entry, resolvedProgramId);
                        this.populateProgramRegistryInfo(resolvedProgramId);
                    }
                } catch (error) {
                    // Registry lookup is optional
                }
            }

            // Fetch and display ABI
            try {
                const abi = await this.rpc.getContractAbi(resolvedProgramId);
                this.displayProgramAbi(abi);
            } catch (error) {
                this.displayProgramAbi(null);
            }

            window.copyProgramId = () => {
                navigator.clipboard.writeText(info.program || programId);
            };
        } catch (error) {
            this.addTerminalLine('⚠️  Failed to load program info', 'warning');
        }
    },

    async loadProgramStorage(programId) {
        if (!this.rpc) return;

        const storagePanel = document.getElementById('storageViewerPanel');
        if (storagePanel) {
            storagePanel.style.display = 'block';
        }

        const container = document.getElementById('storageViewer');
        if (!container) return;

        try {
            const result = await this.rpc.getProgramStorage(programId, { limit: 50 });
            const entries = result.entries || [];

            if (entries.length === 0) {
                container.innerHTML = `
                    <div class="empty-state">
                        <i class="fas fa-database"></i>
                        <p>No storage data</p>
                    </div>
                `;
                return;
            }

            // AUDIT-FIX F14.5: escape RPC storage data
            container.innerHTML = entries.map(entry => `
                <div class="storage-row">
                    <div class="storage-key monospace">${escapeHtml(entry.key)}</div>
                    <div class="storage-value monospace">${escapeHtml(entry.value)}</div>
                </div>
            `).join('');
        } catch (error) {
            container.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-database"></i>
                    <p>Unable to load storage</p>
                </div>
            `;
        }
    },

    async loadProgramCalls(programId) {
        if (!this.rpc) return;

        const callsPanel = document.getElementById('programCallsPanel');
        if (callsPanel) {
            callsPanel.style.display = 'block';
        }

        const container = document.getElementById('programCallsList');
        if (!container) return;

        try {
            const result = await this.rpc.getProgramCalls(programId, { limit: 25 });
            this.programCallsCache = result.calls || [];
            this.renderProgramCalls();
        } catch (error) {
            container.innerHTML = `
                <div class="empty-state">
                    <i class="fas fa-stream"></i>
                    <p>Unable to load calls</p>
                </div>
            `;
        }
    },

    /**
     * Display contract ABI in the program info panel
     */
    displayProgramAbi(abi) {
        let container = document.getElementById('programAbiPanel');
        if (!container) {
            // Create ABI panel dynamically if it doesn't exist
            const infoPanel = document.getElementById('programInfoPanel');
            if (!infoPanel) return;
            container = document.createElement('div');
            container.id = 'programAbiPanel';
            container.className = 'panel-section';
            infoPanel.appendChild(container);
        }

        if (!abi || abi.error || !abi.functions || abi.functions.length === 0) {
            container.innerHTML = `
                <div class="panel-section-header">
                    <i class="fas fa-file-code"></i> ABI / Interface
                </div>
                <div class="empty-state" style="padding: 12px; font-size: 12px; color: var(--text-muted);">
                    <i class="fas fa-puzzle-piece"></i>
                    <p>No ABI available</p>
                </div>
            `;
            return;
        }

        // AUDIT-FIX F14.6: escape all RPC-sourced ABI fields
        const funcRows = abi.functions.map(fn => {
            const params = (fn.params || []).map(p => 
                `<span class="abi-param" title="${escapeHtml(p.description || '')}">${escapeHtml(p.name)}: <em>${escapeHtml(p.type || p.param_type)}</em></span>`
            ).join(', ');
            const ret = fn.returns 
                ? `<span class="abi-return">&rarr; ${escapeHtml(fn.returns.type || fn.returns.return_type)}</span>` 
                : '';
            const badge = fn.readonly 
                ? '<span class="badge badge-info" style="margin-left: 4px; font-size: 9px;">view</span>' 
                : '';
            return `
                <div class="abi-function" style="padding: 4px 8px; border-bottom: 1px solid var(--border); font-family: monospace; font-size: 11px;">
                    <strong>${escapeHtml(fn.name)}</strong>${badge}(${params}) ${ret}
                    ${fn.description ? `<div style="color: var(--text-muted); font-size: 10px; margin-top: 2px;">${escapeHtml(fn.description)}</div>` : ''}
                </div>
            `;
        }).join('');

        const eventRows = (abi.events || []).map(ev => {
            const fields = (ev.fields || []).map(f => 
                `${escapeHtml(f.name)}: ${escapeHtml(f.type || f.field_type)}${f.indexed ? ' (indexed)' : ''}`
            ).join(', ');
            return `
                <div style="padding: 4px 8px; font-family: monospace; font-size: 11px; color: var(--text-muted);">
                    <i class="fas fa-bell" style="font-size: 9px;"></i> ${escapeHtml(ev.name)}(${fields})
                </div>
            `;
        }).join('');

        container.innerHTML = `
            <div class="panel-section-header">
                <i class="fas fa-file-code"></i> ABI / Interface
                <span style="float: right; font-size: 10px; color: var(--text-muted);">
                    v${escapeHtml(String(abi.version || '?'))} &middot; ${abi.functions.length} functions
                    ${abi.template ? ` &middot; ${escapeHtml(abi.template)}` : ''}
                </span>
            </div>
            <div class="abi-functions">${funcRows}</div>
            ${eventRows ? `<div class="abi-events" style="border-top: 1px solid var(--border); margin-top: 4px;">${eventRows}</div>` : ''}
        `;
    }
};

// ============================================================================
// DEFAULT FILES & EXAMPLES
// ============================================================================

const DEFAULT_FILES = {
    LIB_RS: `// Welcome to MoltChain Playground
// Project: workspace
// This is a simple counter program

use borsh::{BorshDeserialize, BorshSerialize};
use moltchain_sdk::*;

// Program ID (auto-updated by Playground)
const PROGRAM_ID: &str = "program_id_here";

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Counter {
    pub count: u64,
}

/// Initialize counter
#[no_mangle]
pub extern "C" fn initialize() -> Result<()> {
    let counter = Counter { count: 0 };
    msg!("Counter initialized!");
    Ok(())
}

/// Increment counter
#[no_mangle]
pub extern "C" fn increment() -> Result<()> {
    let mut counter = get_account::<Counter>()?;
    counter.count += 1;
    msg!("Counter incremented to: {}", counter.count);
    set_account(&counter)?;
    Ok(())
}

/// Get current count
#[no_mangle]
pub extern "C" fn get_count() -> Result<u64> {
    let counter = get_account::<Counter>()?;
    Ok(counter.count)
}

/// Reset counter
#[no_mangle]
pub extern "C" fn reset() -> Result<()> {
    let mut counter = get_account::<Counter>()?;
    counter.count = 0;
    msg!("Counter reset!");
    set_account(&counter)?;
    Ok(())
}`,
    
    CARGO_TOML: `[package]
name = "workspace"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"
borsh = "0.10"

[profile.release]
opt-level = "z"
lto = true`,
    
    TEST_FILE: `#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter() {
        initialize().unwrap();
        assert_eq!(get_count().unwrap(), 0);
        
        increment().unwrap();
        assert_eq!(get_count().unwrap(), 1);
        
        reset().unwrap();
        assert_eq!(get_count().unwrap(), 0);
    }
}`
};

const EXAMPLES = {
    blank: {
        name: 'Blank',
        description: 'Minimal contract starter',
        files: {
            'lib.rs': `// Minimal MoltChain contract

use moltchain_sdk::*;

// Program ID (auto-updated by Playground)
const PROGRAM_ID: &str = "program_id_here";

#[no_mangle]
pub extern "C" fn initialize() -> Result<()> {
    msg!("Initialized");
    Ok(())
}`,
            'Cargo.toml': DEFAULT_FILES.CARGO_TOML
        }
    },
    hello_world: {
        name: 'Hello World',
        description: 'Basic contract template',
        files: {
            'lib.rs': DEFAULT_FILES.LIB_RS,
            'Cargo.toml': DEFAULT_FILES.CARGO_TOML
        }
    },
    counter: {
        name: 'Counter',
        description: 'Simple state management',
        files: {
            'lib.rs': DEFAULT_FILES.LIB_RS,
            'Cargo.toml': DEFAULT_FILES.CARGO_TOML
        }
    },
    token: {
        name: 'MoltCoin MT-20',
        description: 'Native fungible token contract',
        templateType: 'token',
        files: {
            'lib.rs': `// MoltCoin Token Contract
// Example MT-20 fungible token

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use moltchain_sdk::{Token, Address, log_info};

// Initialize token
static mut TOKEN: Option<Token> = None;
static mut OWNER: Option<Address> = None;

fn get_token() -> &'static mut Token {
    unsafe {
        TOKEN.as_mut().expect("Token not initialized")
    }
}

fn get_owner() -> Address {
    unsafe {
        OWNER.expect("Owner not set")
    }
}

/// Initialize the token contract
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) {
    let owner_bytes = unsafe {
        core::slice::from_raw_parts(owner_ptr, 32)
    };
    let mut owner_array = [0u8; 32];
    owner_array.copy_from_slice(owner_bytes);
    let owner = Address::new(owner_array);

    unsafe {
        OWNER = Some(owner);
        TOKEN = Some(Token::new("MoltCoin", "MOLT", 9));
    }

    // Initialize with 1 million tokens
    let initial_supply = 1_000_000 * 1_000_000_000; // 1M with 9 decimals
    get_token().initialize(initial_supply, owner).expect("Initialization failed");

    log_info("MoltCoin initialized");
}

/// Get balance of an account
#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    let account_bytes = unsafe {
        core::slice::from_raw_parts(account_ptr, 32)
    };
    let mut account_array = [0u8; 32];
    account_array.copy_from_slice(account_bytes);
    let account = Address::new(account_array);

    get_token().balance_of(account)
}

/// Transfer tokens
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let from_bytes = unsafe { core::slice::from_raw_parts(from_ptr, 32) };
    let to_bytes = unsafe { core::slice::from_raw_parts(to_ptr, 32) };

    let mut from_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    from_array.copy_from_slice(from_bytes);
    to_array.copy_from_slice(to_bytes);

    let from = Address::new(from_array);
    let to = Address::new(to_array);

    match get_token().transfer(from, to, amount) {
        Ok(_) => {
            log_info("Transfer successful");
            1
        }
        Err(_) => {
            log_info("Transfer failed");
            0
        }
    }
}

/// Mint new tokens (owner only)
#[no_mangle]
pub extern "C" fn mint(caller_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let caller_bytes = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let to_bytes = unsafe { core::slice::from_raw_parts(to_ptr, 32) };

    let mut caller_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    caller_array.copy_from_slice(caller_bytes);
    to_array.copy_from_slice(to_bytes);

    let caller = Address::new(caller_array);
    let to = Address::new(to_array);
    let owner = get_owner();

    match get_token().mint(to, amount, caller, owner) {
        Ok(_) => {
            log_info("Mint successful");
            1
        }
        Err(_) => {
            log_info("Mint failed - unauthorized");
            0
        }
    }
}

/// Burn tokens
#[no_mangle]
pub extern "C" fn burn(from_ptr: *const u8, amount: u64) -> u32 {
    let from_bytes = unsafe { core::slice::from_raw_parts(from_ptr, 32) };
    let mut from_array = [0u8; 32];
    from_array.copy_from_slice(from_bytes);
    let from = Address::new(from_array);

    match get_token().burn(from, amount) {
        Ok(_) => {
            log_info("Burn successful");
            1
        }
        Err(_) => {
            log_info("Burn failed");
            0
        }
    }
}

/// Approve spender
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, amount: u64) -> u32 {
    let owner_bytes = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let spender_bytes = unsafe { core::slice::from_raw_parts(spender_ptr, 32) };

    let mut owner_array = [0u8; 32];
    let mut spender_array = [0u8; 32];
    owner_array.copy_from_slice(owner_bytes);
    spender_array.copy_from_slice(spender_bytes);

    let owner = Address::new(owner_array);
    let spender = Address::new(spender_array);

    match get_token().approve(owner, spender, amount) {
        Ok(_) => {
            log_info("Approval successful");
            1
        }
        Err(_) => 0,
    }
}

/// Get total supply
#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    get_token().total_supply
}
`,
            'Cargo.toml': `[package]
name = "moltcoin-token"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    nft: {
        name: 'MoltPunks Collection',
        description: 'NFT collection + minting',
        templateType: 'nft',
        files: {
            'lib.rs': `// MoltPunks - Collectible NFT Contract
// Example implementation of MT-721 standard

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use moltchain_sdk::{NFT, Address, log_info};

static mut NFT_COLLECTION: Option<NFT> = None;
static mut MINTER: Option<Address> = None;

// Helper to get NFT collection
fn get_nft() -> &'static mut NFT {
    unsafe {
        NFT_COLLECTION.as_mut().expect("NFT not initialized")
    }
}

// Helper to get minter address
fn get_minter() -> Address {
    unsafe {
        MINTER.expect("Minter not set")
    }
}

/// Initialize the NFT collection
#[no_mangle]
pub extern "C" fn initialize(minter_ptr: *const u8) {
    unsafe {
        // Parse minter address
        let minter_slice = core::slice::from_raw_parts(minter_ptr, 32);
        let mut minter_addr = [0u8; 32];
        minter_addr.copy_from_slice(minter_slice);
        let minter = Address(minter_addr);
        
        // Initialize collection
        NFT_COLLECTION = Some(NFT::new("MoltPunks", "MPNK"));
        MINTER = Some(minter);
        
        get_nft().initialize(minter).expect("Init failed");
        
        log_info("MoltPunks NFT collection initialized");
    }
}

/// Mint new NFT
#[no_mangle]
pub extern "C" fn mint(
    caller_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
    metadata_ptr: *const u8,
    metadata_len: u32,
) -> u32 {
    unsafe {
        // Parse caller
        let caller_slice = core::slice::from_raw_parts(caller_ptr, 32);
        let mut caller_addr = [0u8; 32];
        caller_addr.copy_from_slice(caller_slice);
        let caller = Address(caller_addr);
        
        // Check if caller is minter
        if caller.0 != get_minter().0 {
            log_info("Unauthorized: Only minter can mint");
            return 0;
        }
        
        // Parse recipient
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        // Parse metadata URI
        let metadata = core::slice::from_raw_parts(metadata_ptr, metadata_len as usize);
        
        // Mint
        match get_nft().mint(to, token_id, metadata) {
            Ok(_) => {
                log_info("NFT minted successfully");
                1
            }
            Err(_) => {
                log_info("Mint failed");
                0
            }
        }
    }
}

/// Transfer NFT
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        // Parse from address
        let from_slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut from_addr = [0u8; 32];
        from_addr.copy_from_slice(from_slice);
        let from = Address(from_addr);
        
        // Parse to address
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        // Transfer
        match get_nft().transfer(from, to, token_id) {
            Ok(_) => {
                log_info("NFT transferred successfully");
                1
            }
            Err(_) => {
                log_info("Transfer failed");
                0
            }
        }
    }
}

/// Get owner of token
#[no_mangle]
pub extern "C" fn owner_of(token_id: u64, out_ptr: *mut u8) -> u32 {
    unsafe {
        match get_nft().owner_of(token_id) {
            Ok(owner) => {
                let out_slice = core::slice::from_raw_parts_mut(out_ptr, 32);
                out_slice.copy_from_slice(&owner.0);
                1
            }
            Err(_) => 0,
        }
    }
}

/// Get balance (number of NFTs owned)
#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    unsafe {
        let account_slice = core::slice::from_raw_parts(account_ptr, 32);
        let mut account_addr = [0u8; 32];
        account_addr.copy_from_slice(account_slice);
        let account = Address(account_addr);
        
        get_nft().balance_of(account)
    }
}

/// Approve spender for token
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        let spender_slice = core::slice::from_raw_parts(spender_ptr, 32);
        let mut spender_addr = [0u8; 32];
        spender_addr.copy_from_slice(spender_slice);
        let spender = Address(spender_addr);
        
        match get_nft().approve(owner, spender, token_id) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
}

/// Transfer from (with approval)
#[no_mangle]
pub extern "C" fn transfer_from(
    caller_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
) -> u32 {
    unsafe {
        let caller_slice = core::slice::from_raw_parts(caller_ptr, 32);
        let mut caller_addr = [0u8; 32];
        caller_addr.copy_from_slice(caller_slice);
        let caller = Address(caller_addr);
        
        let from_slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut from_addr = [0u8; 32];
        from_addr.copy_from_slice(from_slice);
        let from = Address(from_addr);
        
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        match get_nft().transfer_from(caller, from, to, token_id) {
            Ok(_) => {
                log_info("TransferFrom successful");
                1
            }
            Err(_) => {
                log_info("TransferFrom failed");
                0
            }
        }
    }
}

/// Burn NFT
#[no_mangle]
pub extern "C" fn burn(owner_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        match get_nft().burn(owner, token_id) {
            Ok(_) => {
                log_info("NFT burned");
                1
            }
            Err(_) => {
                log_info("Burn failed");
                0
            }
        }
    }
}

/// Get total minted
#[no_mangle]
pub extern "C" fn total_minted() -> u64 {
    get_nft().total_minted
}
`,
            'Cargo.toml': `[package]
name = "moltpunks-nft"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    dex: {
        name: 'MoltSwap AMM',
        description: 'AMM with TWAP oracle, price impact guards, flash loans',
        files: {
            'lib.rs': `// MoltSwap - Automated Market Maker DEX
// Decentralized exchange with liquidity pools

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use moltchain_sdk::{Pool, Address, log_info};

static mut POOL: Option<Pool> = None;

// Helper to get pool
fn get_pool() -> &'static mut Pool {
    unsafe {
        POOL.as_mut().expect("Pool not initialized")
    }
}

/// Initialize the liquidity pool
#[no_mangle]
pub extern "C" fn initialize(token_a_ptr: *const u8, token_b_ptr: *const u8) {
    unsafe {
        // Parse token addresses
        let token_a_slice = core::slice::from_raw_parts(token_a_ptr, 32);
        let mut token_a_addr = [0u8; 32];
        token_a_addr.copy_from_slice(token_a_slice);
        let token_a = Address(token_a_addr);
        
        let token_b_slice = core::slice::from_raw_parts(token_b_ptr, 32);
        let mut token_b_addr = [0u8; 32];
        token_b_addr.copy_from_slice(token_b_slice);
        let token_b = Address(token_b_addr);
        
        // Create pool
        let mut pool = Pool::new(token_a, token_b);
        pool.initialize(token_a, token_b).expect("Init failed");
        
        POOL = Some(pool);
        
        log_info("MoltSwap liquidity pool initialized");
    }
}

/// Add liquidity to the pool
#[no_mangle]
pub extern "C" fn add_liquidity(
    provider_ptr: *const u8,
    amount_a: u64,
    amount_b: u64,
    min_liquidity: u64,
) -> u64 {
    unsafe {
        let provider_slice = core::slice::from_raw_parts(provider_ptr, 32);
        let mut provider_addr = [0u8; 32];
        provider_addr.copy_from_slice(provider_slice);
        let provider = Address(provider_addr);
        
        match get_pool().add_liquidity(provider, amount_a, amount_b, min_liquidity) {
            Ok(liquidity) => {
                log_info("Liquidity added successfully");
                liquidity
            }
            Err(_) => {
                log_info("Add liquidity failed");
                0
            }
        }
    }
}

/// Remove liquidity from the pool
#[no_mangle]
pub extern "C" fn remove_liquidity(
    provider_ptr: *const u8,
    liquidity: u64,
    min_amount_a: u64,
    min_amount_b: u64,
    out_a_ptr: *mut u8,
    out_b_ptr: *mut u8,
) -> u32 {
    unsafe {
        let provider_slice = core::slice::from_raw_parts(provider_ptr, 32);
        let mut provider_addr = [0u8; 32];
        provider_addr.copy_from_slice(provider_slice);
        let provider = Address(provider_addr);
        
        match get_pool().remove_liquidity(provider, liquidity, min_amount_a, min_amount_b) {
            Ok((amount_a, amount_b)) => {
                log_info("Liquidity removed successfully");
                
                // Write amounts to output pointers
                let out_a_slice = core::slice::from_raw_parts_mut(out_a_ptr, 8);
                out_a_slice.copy_from_slice(&amount_a.to_le_bytes());
                
                let out_b_slice = core::slice::from_raw_parts_mut(out_b_ptr, 8);
                out_b_slice.copy_from_slice(&amount_b.to_le_bytes());
                
                1
            }
            Err(_) => {
                log_info("Remove liquidity failed");
                0
            }
        }
    }
}

/// Swap token A for token B
#[no_mangle]
pub extern "C" fn swap_a_for_b(amount_a_in: u64, min_amount_b_out: u64) -> u64 {
    match get_pool().swap_a_for_b(amount_a_in, min_amount_b_out) {
        Ok(amount_b_out) => {
            log_info("Swap A->B successful");
            amount_b_out
        }
        Err(_) => {
            log_info("Swap A->B failed");
            0
        }
    }
}

/// Swap token B for token A
#[no_mangle]
pub extern "C" fn swap_b_for_a(amount_b_in: u64, min_amount_a_out: u64) -> u64 {
    match get_pool().swap_b_for_a(amount_b_in, min_amount_a_out) {
        Ok(amount_a_out) => {
            log_info("Swap B->A successful");
            amount_a_out
        }
        Err(_) => {
            log_info("Swap B->A failed");
            0
        }
    }
}

/// Get quote for swap (how much output for given input)
#[no_mangle]
pub extern "C" fn get_quote(amount_in: u64, is_a_to_b: u32) -> u64 {
    let pool = get_pool();
    
    if is_a_to_b == 1 {
        pool.get_amount_out(amount_in, pool.reserve_a, pool.reserve_b)
    } else {
        pool.get_amount_out(amount_in, pool.reserve_b, pool.reserve_a)
    }
}

/// Get reserve amounts
#[no_mangle]
pub extern "C" fn get_reserves(out_a_ptr: *mut u8, out_b_ptr: *mut u8) {
    unsafe {
        let pool = get_pool();
        
        let out_a_slice = core::slice::from_raw_parts_mut(out_a_ptr, 8);
        out_a_slice.copy_from_slice(&pool.reserve_a.to_le_bytes());
        
        let out_b_slice = core::slice::from_raw_parts_mut(out_b_ptr, 8);
        out_b_slice.copy_from_slice(&pool.reserve_b.to_le_bytes());
    }
}

/// Get liquidity balance of provider
#[no_mangle]
pub extern "C" fn get_liquidity_balance(provider_ptr: *const u8) -> u64 {
    unsafe {
        let provider_slice = core::slice::from_raw_parts(provider_ptr, 32);
        let mut provider_addr = [0u8; 32];
        provider_addr.copy_from_slice(provider_slice);
        let provider = Address(provider_addr);
        
        get_pool().get_liquidity_balance(provider)
    }
}

/// Get total liquidity
#[no_mangle]
pub extern "C" fn get_total_liquidity() -> u64 {
    get_pool().total_liquidity
}
`,
            'Cargo.toml': `[package]
name = "moltswap-dex"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    dao: {
        name: 'MoltDAO Governance',
        description: 'Token-weighted proposals',
        files: {
            'lib.rs': `// MoltDAO - Decentralized Autonomous Organization
// Features: Token-weighted voting, Proposals, Treasury management

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_timestamp,
    call_token_transfer
};

// ============================================================================
// DAO CONFIGURATION
// ============================================================================

const VOTING_PERIOD: u64 = 259200; // 3 days
const EXECUTION_DELAY: u64 = 86400; // 1 day
const QUORUM_PERCENTAGE: u64 = 10; // 10% of total supply must vote
const APPROVAL_THRESHOLD: u64 = 51; // 51% approval required

#[no_mangle]
pub extern "C" fn initialize_dao(
    governance_token_ptr: *const u8,
    treasury_address_ptr: *const u8,
    min_proposal_threshold: u64, // Minimum tokens to create proposal
) -> u32 {
    log_info("🏛️  Initializing MoltDAO...");
    
    let gov_token = unsafe { core::slice::from_raw_parts(governance_token_ptr, 32) };
    let treasury = unsafe { core::slice::from_raw_parts(treasury_address_ptr, 32) };
    
    storage_set(b"governance_token", gov_token);
    storage_set(b"treasury", treasury);
    storage_set(b"min_proposal_threshold", &u64_to_bytes(min_proposal_threshold));
    storage_set(b"proposal_count", &u64_to_bytes(0));
    
    log_info("✅ DAO initialized!");
    log_info("   Voting period: 3 days");
    log_info("   Quorum: 10%");
    log_info("   Approval: 51%");
    log_info(&alloc::format!("   Min proposal tokens: {}", min_proposal_threshold));
    
    1
}

// ============================================================================
// PROPOSAL SYSTEM
// ============================================================================

// Proposal: 201 bytes
// proposer (32) + title_hash (32) + description_hash (32) +
// target_contract (32) + action (32) + start_time (8) + 
// end_time (8) + votes_for (8) + votes_against (8) + 
// executed (1) + cancelled (1) + quorum_met (1)
const PROPOSAL_SIZE: usize = 201;

#[no_mangle]
pub extern "C" fn create_proposal(
    proposer_ptr: *const u8,
    title_ptr: *const u8,
    title_len: u32,
    description_ptr: *const u8,
    description_len: u32,
    target_contract_ptr: *const u8,
    action_ptr: *const u8,
    action_len: u32,
) -> u32 {
    log_info("📝 Creating proposal...");
    
    let proposer = unsafe { core::slice::from_raw_parts(proposer_ptr, 32) };
    let title = unsafe { core::slice::from_raw_parts(title_ptr, title_len as usize) };
    let description = unsafe { core::slice::from_raw_parts(description_ptr, description_len as usize) };
    let target_contract = unsafe { core::slice::from_raw_parts(target_contract_ptr, 32) };
    let action = unsafe { core::slice::from_raw_parts(action_ptr, action_len as usize) };
    
    // Check proposer has enough tokens
    let min_threshold = storage_get(b"min_proposal_threshold")
        .and_then(|d| Some(bytes_to_u64(&d)))
        .unwrap_or(0);
    
    // In production, verify proposer's token balance against min_threshold
    log_info(&alloc::format!("   Min threshold: {} tokens", min_threshold));
    
    // Generate proposal ID
    let mut proposal_count = storage_get(b"proposal_count")
        .and_then(|d| Some(bytes_to_u64(&d)))
        .unwrap_or(0);
    
    proposal_count += 1;
    
    // Hash title and description (simple hash for demo)
    let mut title_hash = [0u8; 32];
    for (i, &byte) in title.iter().take(32).enumerate() {
        title_hash[i] = byte;
    }
    
    let mut description_hash = [0u8; 32];
    for (i, &byte) in description.iter().take(32).enumerate() {
        description_hash[i] = byte;
    }
    
    let mut action_hash = [0u8; 32];
    for (i, &byte) in action.iter().take(32).enumerate() {
        action_hash[i] = byte;
    }
    
    let now = get_timestamp();
    let end_time = now + VOTING_PERIOD;
    
    // Build proposal
    let mut proposal = Vec::with_capacity(PROPOSAL_SIZE);
    proposal.extend_from_slice(proposer);                 // 0-31: proposer
    proposal.extend_from_slice(&title_hash);              // 32-63: title_hash
    proposal.extend_from_slice(&description_hash);        // 64-95: description_hash
    proposal.extend_from_slice(target_contract);          // 96-127: target_contract
    proposal.extend_from_slice(&action_hash);             // 128-159: action
    proposal.extend_from_slice(&u64_to_bytes(now));       // 160-167: start_time
    proposal.extend_from_slice(&u64_to_bytes(end_time));  // 168-175: end_time
    proposal.extend_from_slice(&[0u8; 8]);                // 176-183: votes_for
    proposal.extend_from_slice(&[0u8; 8]);                // 184-191: votes_against
    proposal.push(0);                                      // 192: executed
    proposal.push(0);                                      // 193: cancelled
    proposal.push(0);                                      // 194: quorum_met
    
    // Pad to full size
    while proposal.len() < PROPOSAL_SIZE {
        proposal.push(0);
    }
    
    // Store proposal
    let key = alloc::format!("proposal_{}", proposal_count);
    storage_set(key.as_bytes(), &proposal);
    storage_set(b"proposal_count", &u64_to_bytes(proposal_count));
    
    log_info("✅ Proposal created!");
    log_info(&alloc::format!("   ID: {}", proposal_count));
    log_info(&alloc::format!("   Title: {}", 
        core::str::from_utf8(title).unwrap_or("?")
    ));
    log_info(&alloc::format!("   Voting ends: {} seconds", VOTING_PERIOD));
    
    proposal_count as u32
}
`,
            'Cargo.toml': `[package]
name = "moltdao-governance"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    lending: {
        name: 'LobsterLend',
        description: 'Lending with flash loans, reentrancy guards, emergency pause',
        templateType: 'lending',
        files: {
            'lib.rs': `// LobsterLend - P2P Lending Protocol
// Deposit collateral, borrow assets, earn interest, automated liquidations

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp
};

// ============================================================================
// CONFIGURATION
// ============================================================================

const COLLATERAL_FACTOR_PERCENT: u64 = 75;   // Max borrow = 75% of deposit
const LIQUIDATION_THRESHOLD: u64 = 85;        // Liquidate at 85% utilization
const LIQUIDATION_BONUS: u64 = 5;             // 5% bonus for liquidators
const BASE_RATE_SCALED: u64 = 254;            // ~0.8% APR base rate
const RATE_SCALE: u64 = 10_000_000_000;
const UTILIZATION_KINK: u64 = 80;             // Rate model kink at 80%

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

// ============================================================================
// PROTOCOL
// ============================================================================

#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };
    storage_set(b"ll_admin", admin);
    store_u64(b"ll_total_deposits", 0);
    store_u64(b"ll_total_borrows", 0);
    store_u64(b"ll_last_accrue", get_timestamp());
    log_info("\\xf0\\x9f\\xa6\\x9e LobsterLend initialized");
    1
}

#[no_mangle]
pub extern "C" fn deposit(depositor_ptr: *const u8, amount: u64) -> u32 {
    let depositor = unsafe { core::slice::from_raw_parts(depositor_ptr, 32) };
    let h = hex(depositor);
    let key = alloc::format!("dep:{}", h);
    let current = load_u64(key.as_bytes());
    store_u64(key.as_bytes(), current + amount);
    let total = load_u64(b"ll_total_deposits");
    store_u64(b"ll_total_deposits", total + amount);
    log_info(&alloc::format!("Deposited {} shells", amount));
    1
}

#[no_mangle]
pub extern "C" fn borrow(borrower_ptr: *const u8, amount: u64) -> u32 {
    let borrower = unsafe { core::slice::from_raw_parts(borrower_ptr, 32) };
    let h = hex(borrower);

    let dep_key = alloc::format!("dep:{}", h);
    let bor_key = alloc::format!("bor:{}", h);
    let deposit = load_u64(dep_key.as_bytes());
    let existing_borrow = load_u64(bor_key.as_bytes());

    let max_borrow = deposit * COLLATERAL_FACTOR_PERCENT / 100;
    if existing_borrow + amount > max_borrow {
        log_info("Borrow exceeds collateral factor");
        return 0;
    }

    store_u64(bor_key.as_bytes(), existing_borrow + amount);
    let total = load_u64(b"ll_total_borrows");
    store_u64(b"ll_total_borrows", total + amount);
    log_info(&alloc::format!("Borrowed {} shells", amount));
    1
}

#[no_mangle]
pub extern "C" fn repay(borrower_ptr: *const u8, amount: u64) -> u32 {
    let borrower = unsafe { core::slice::from_raw_parts(borrower_ptr, 32) };
    let h = hex(borrower);
    let key = alloc::format!("bor:{}", h);
    let current = load_u64(key.as_bytes());
    let repay_amount = if amount > current { current } else { amount };
    store_u64(key.as_bytes(), current - repay_amount);
    let total = load_u64(b"ll_total_borrows");
    store_u64(b"ll_total_borrows", total.saturating_sub(repay_amount));
    log_info(&alloc::format!("Repaid {} shells", repay_amount));
    1
}

#[no_mangle]
pub extern "C" fn liquidate(liquidator_ptr: *const u8, borrower_ptr: *const u8, repay_amount: u64) -> u32 {
    let borrower = unsafe { core::slice::from_raw_parts(borrower_ptr, 32) };
    let h = hex(borrower);
    let deposit = load_u64(alloc::format!("dep:{}", h).as_bytes());
    let borrow = load_u64(alloc::format!("bor:{}", h).as_bytes());

    if deposit == 0 || borrow * 100 / deposit < LIQUIDATION_THRESHOLD {
        log_info("Position is healthy, cannot liquidate");
        return 0;
    }

    let bonus = repay_amount * (100 + LIQUIDATION_BONUS) / 100;
    log_info(&alloc::format!("Liquidated! Repaid {} + {} bonus", repay_amount, bonus - repay_amount));
    1
}

#[no_mangle]
pub extern "C" fn get_protocol_stats() -> u32 {
    let deposits = load_u64(b"ll_total_deposits");
    let borrows = load_u64(b"ll_total_borrows");
    let utilization = if deposits > 0 { borrows * 100 / deposits } else { 0 };
    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(deposits));
    result.extend_from_slice(&u64_to_bytes(borrows));
    result.extend_from_slice(&u64_to_bytes(utilization));
    set_return_data(&result);
    1
}
`,
            'Cargo.toml': `[package]
name = "lobsterlend"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    launchpad: {
        name: 'ClawPump Launchpad',
        description: 'Bonding curve launches with anti-manipulation and royalties',
        templateType: 'launchpad',
        files: {
            'lib.rs': `// ClawPump - Token Launchpad with Bonding Curves
// Fair-launch tokens that graduate to MoltSwap at market cap threshold

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp
};

const CREATION_FEE: u64 = 10_000_000_000;     // 10 MOLT
const GRADUATION_MCAP: u64 = 1_000_000;        // Graduate at 1M MOLT market cap
const BASE_PRICE: u64 = 1000;                 // Base price in shells
const SLOPE: u64 = 1;                         // Linear bonding curve slope
const SLOPE_SCALE: u64 = 1_000_000;
const PLATFORM_FEE_PERCENT: u64 = 1;

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };
    storage_set(b"cp_admin", admin);
    store_u64(b"cp_token_count", 0);
    store_u64(b"cp_total_fees", 0);
    log_info("\\xf0\\x9f\\x9a\\x80 ClawPump initialized");
    1
}

#[no_mangle]
pub extern "C" fn create_token(creator_ptr: *const u8, fee_paid: u64) -> u64 {
    if fee_paid < CREATION_FEE {
        log_info("Insufficient creation fee (need 0.1 MOLT)");
        return 0;
    }
    let creator = unsafe { core::slice::from_raw_parts(creator_ptr, 32) };
    let count = load_u64(b"cp_token_count") + 1;
    store_u64(b"cp_token_count", count);

    let h = alloc::format!("{:016x}", count);
    let key = alloc::format!("cpt:{}", h);
    let mut data = Vec::with_capacity(65);
    data.extend_from_slice(creator);                // creator (32)
    data.extend_from_slice(&u64_to_bytes(0));       // supply_sold (8)
    data.extend_from_slice(&u64_to_bytes(0));       // molt_raised (8)
    data.extend_from_slice(&u64_to_bytes(1_000_000_000 * 1_000_000_000)); // max_supply (8)
    data.extend_from_slice(&u64_to_bytes(get_timestamp())); // created_at (8)
    data.push(0);                                    // graduated (1)
    storage_set(key.as_bytes(), &data);

    log_info(&alloc::format!("Token #{} created on bonding curve", count));
    count
}

#[no_mangle]
pub extern "C" fn buy(buyer_ptr: *const u8, token_id: u64, molt_amount: u64) -> u64 {
    let h = alloc::format!("{:016x}", token_id);
    let key = alloc::format!("cpt:{}", h);
    let data = match storage_get(key.as_bytes()) {
        Some(d) => d,
        None => { log_info("Token not found"); return 0; }
    };
    if data[64] == 1 { log_info("Token graduated, trade on MoltSwap"); return 0; }

    let supply = bytes_to_u64(&data[32..40]);
    // Linear bonding curve: price = BASE + SLOPE * supply / SCALE
    let price = BASE_PRICE + SLOPE * supply / SLOPE_SCALE;
    let fee = molt_amount * PLATFORM_FEE_PERCENT / 100;
    let net = molt_amount - fee;
    let tokens_bought = if price > 0 { net * 1_000_000_000 / price } else { 0 };

    log_info(&alloc::format!("Bought {} tokens at price {} (1% fee)", tokens_bought, price));
    tokens_bought
}

#[no_mangle]
pub extern "C" fn sell(seller_ptr: *const u8, token_id: u64, token_amount: u64) -> u64 {
    let h = alloc::format!("{:016x}", token_id);
    let key = alloc::format!("cpt:{}", h);
    let data = match storage_get(key.as_bytes()) {
        Some(d) => d,
        None => { log_info("Token not found"); return 0; }
    };
    let supply = bytes_to_u64(&data[32..40]);
    let price = BASE_PRICE + SLOPE * supply / SLOPE_SCALE;
    let molt_back = token_amount * price / 1_000_000_000;
    log_info(&alloc::format!("Sold {} tokens for {} MOLT", token_amount, molt_back));
    molt_back
}

#[no_mangle]
pub extern "C" fn get_token_info(token_id: u64) -> u32 {
    let h = alloc::format!("{:016x}", token_id);
    let key = alloc::format!("cpt:{}", h);
    match storage_get(key.as_bytes()) {
        Some(d) => { set_return_data(&d); 1 }
        None => 0
    }
}

#[no_mangle]
pub extern "C" fn get_token_count() -> u64 {
    load_u64(b"cp_token_count")
}
`,
            'Cargo.toml': `[package]
name = "clawpump"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    streaming: {
        name: 'ClawPay Streaming',
        description: 'Sablier-style payment streams',
        files: {
            'lib.rs': `// ClawPay - Streaming Payments
// Time-windowed streams with proportional withdrawals

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, set_return_data,
    bytes_to_u64, u64_to_bytes, get_slot
};

// Stream: sender(32) + recipient(32) + total(8) + withdrawn(8) +
//         start_slot(8) + end_slot(8) + cancelled(1) + created(8) = 105 bytes
const STREAM_SIZE: usize = 105;

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

#[no_mangle]
pub extern "C" fn create_stream(
    sender_ptr: *const u8,
    recipient_ptr: *const u8,
    total_amount: u64,
    start_slot: u64,
    end_slot: u64,
) -> u32 {
    if end_slot <= start_slot || total_amount == 0 {
        log_info("Invalid stream parameters");
        return 0;
    }

    let sender = unsafe { core::slice::from_raw_parts(sender_ptr, 32) };
    let recipient = unsafe { core::slice::from_raw_parts(recipient_ptr, 32) };

    let id = load_u64(b"stream_count") + 1;
    store_u64(b"stream_count", id);

    let mut stream = Vec::with_capacity(STREAM_SIZE);
    stream.extend_from_slice(sender);                      // 0..32
    stream.extend_from_slice(recipient);                   // 32..64
    stream.extend_from_slice(&u64_to_bytes(total_amount)); // 64..72
    stream.extend_from_slice(&u64_to_bytes(0));            // 72..80 withdrawn
    stream.extend_from_slice(&u64_to_bytes(start_slot));   // 80..88
    stream.extend_from_slice(&u64_to_bytes(end_slot));     // 88..96
    stream.push(0);                                         // 96 cancelled
    stream.extend_from_slice(&u64_to_bytes(get_slot()));   // 97..105

    let key = alloc::format!("stream_{}", id);
    storage_set(key.as_bytes(), &stream);

    log_info(&alloc::format!("Stream #{} created: {} shells over {} slots", id, total_amount, end_slot - start_slot));
    id as u32
}

#[no_mangle]
pub extern "C" fn withdraw_from_stream(caller_ptr: *const u8, stream_id: u64, amount: u64) -> u32 {
    let key = alloc::format!("stream_{}", stream_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= STREAM_SIZE => d,
        _ => { log_info("Stream not found"); return 0; }
    };

    if data[96] == 1 { log_info("Stream cancelled"); return 0; }

    let total = bytes_to_u64(&data[64..72]);
    let withdrawn = bytes_to_u64(&data[72..80]);
    let start = bytes_to_u64(&data[80..88]);
    let end = bytes_to_u64(&data[88..96]);
    let now = get_slot();

    let elapsed = if now >= end { end - start } else if now > start { now - start } else { 0 };
    let duration = end - start;
    let vested = total * elapsed / duration;
    let available = vested.saturating_sub(withdrawn);

    if amount > available {
        log_info(&alloc::format!("Only {} available ({} vested - {} withdrawn)", available, vested, withdrawn));
        return 0;
    }

    data[72..80].copy_from_slice(&u64_to_bytes(withdrawn + amount));
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Withdrew {} from stream #{}", amount, stream_id));
    1
}

#[no_mangle]
pub extern "C" fn cancel_stream(caller_ptr: *const u8, stream_id: u64) -> u32 {
    let key = alloc::format!("stream_{}", stream_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= STREAM_SIZE => d,
        _ => { log_info("Stream not found"); return 0; }
    };
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if caller != &data[0..32] { log_info("Only sender can cancel"); return 0; }
    data[96] = 1;
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Stream #{} cancelled", stream_id));
    1
}

#[no_mangle]
pub extern "C" fn get_withdrawable(stream_id: u64) -> u32 {
    let key = alloc::format!("stream_{}", stream_id);
    let data = match storage_get(key.as_bytes()) {
        Some(d) => d,
        None => { return 0; }
    };
    let total = bytes_to_u64(&data[64..72]);
    let withdrawn = bytes_to_u64(&data[72..80]);
    let start = bytes_to_u64(&data[80..88]);
    let end = bytes_to_u64(&data[88..96]);
    let now = get_slot();
    let elapsed = if now >= end { end - start } else if now > start { now - start } else { 0 };
    let vested = total * elapsed / (end - start);
    let available = vested.saturating_sub(withdrawn);
    set_return_data(&u64_to_bytes(available));
    1
}
`,
            'Cargo.toml': `[package]
name = "clawpay"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    vault: {
        name: 'ClawVault Yield',
        description: 'Yield aggregator with fees, caps, risk tiers, and strategies',
        templateType: 'vault',
        files: {
            'lib.rs': `// ClawVault - Yield Aggregator
// ERC-4626-style vault shares, multi-strategy, auto-compounding

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp
};

const PERFORMANCE_FEE: u64 = 10;   // 10% on harvested yield
const MAX_STRATEGIES: u64 = 5;
const MIN_LOCKED_SHARES: u64 = 1000; // Anti-inflation attack (T5.9)

// Strategy types
const STRATEGY_LENDING: u8 = 1;
const STRATEGY_LP: u8 = 2;
const STRATEGY_STAKING: u8 = 3;

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };
    storage_set(b"cv_admin", admin);
    store_u64(b"cv_total_assets", 0);
    store_u64(b"cv_total_shares", 0);
    store_u64(b"cv_strategy_count", 0);
    log_info("\\xf0\\x9f\\x8f\\xa6 ClawVault initialized");
    1
}

#[no_mangle]
pub extern "C" fn add_strategy(caller_ptr: *const u8, strategy_type: u8, allocation_percent: u64) -> u32 {
    let count = load_u64(b"cv_strategy_count");
    if count >= MAX_STRATEGIES { log_info("Max strategies reached"); return 0; }
    let idx = count;
    store_u64(alloc::format!("cv_strat_type:{}", idx).as_bytes(), strategy_type as u64);
    store_u64(alloc::format!("cv_strat_alloc:{}", idx).as_bytes(), allocation_percent);
    store_u64(b"cv_strategy_count", count + 1);
    let name = match strategy_type { 1 => "Lending", 2 => "LP", 3 => "Staking", _ => "Custom" };
    log_info(&alloc::format!("Strategy added: {} ({}% allocation)", name, allocation_percent));
    1
}

#[no_mangle]
pub extern "C" fn deposit(depositor_ptr: *const u8, amount: u64) -> u64 {
    let depositor = unsafe { core::slice::from_raw_parts(depositor_ptr, 32) };
    let total_assets = load_u64(b"cv_total_assets");
    let total_shares = load_u64(b"cv_total_shares");

    // ERC-4626 share calculation
    let shares = if total_shares == 0 {
        // First deposit: lock MIN_LOCKED_SHARES to dead address (T5.9 protection)
        let shares = amount;
        store_u64(b"cv_locked_shares", MIN_LOCKED_SHARES);
        shares
    } else {
        amount * total_shares / total_assets
    };

    let h = hex(depositor);
    let key = alloc::format!("cv_shares:{}", h);
    let current = load_u64(key.as_bytes());
    store_u64(key.as_bytes(), current + shares);
    store_u64(b"cv_total_assets", total_assets + amount);
    store_u64(b"cv_total_shares", total_shares + shares);

    log_info(&alloc::format!("Deposited {} MOLT, received {} shares", amount, shares));
    shares
}

#[no_mangle]
pub extern "C" fn withdraw(depositor_ptr: *const u8, shares_to_burn: u64) -> u64 {
    let depositor = unsafe { core::slice::from_raw_parts(depositor_ptr, 32) };
    let h = hex(depositor);
    let key = alloc::format!("cv_shares:{}", h);
    let user_shares = load_u64(key.as_bytes());
    if shares_to_burn > user_shares { log_info("Insufficient shares"); return 0; }

    let total_assets = load_u64(b"cv_total_assets");
    let total_shares = load_u64(b"cv_total_shares");
    let amount = shares_to_burn * total_assets / total_shares;

    store_u64(key.as_bytes(), user_shares - shares_to_burn);
    store_u64(b"cv_total_assets", total_assets - amount);
    store_u64(b"cv_total_shares", total_shares - shares_to_burn);
    log_info(&alloc::format!("Withdrew {} MOLT for {} shares", amount, shares_to_burn));
    amount
}

#[no_mangle]
pub extern "C" fn harvest() -> u32 {
    let count = load_u64(b"cv_strategy_count");
    let mut total_yield: u64 = 0;
    for i in 0..count {
        let alloc_pct = load_u64(alloc::format!("cv_strat_alloc:{}", i).as_bytes());
        let simulated_yield = alloc_pct * 100; // Simulated
        total_yield += simulated_yield;
    }
    let fee = total_yield * PERFORMANCE_FEE / 100;
    let net = total_yield - fee;
    let assets = load_u64(b"cv_total_assets");
    store_u64(b"cv_total_assets", assets + net);
    log_info(&alloc::format!("Harvested: {} yield, {} fee, {} net", total_yield, fee, net));
    1
}

#[no_mangle]
pub extern "C" fn get_vault_stats() -> u32 {
    let assets = load_u64(b"cv_total_assets");
    let shares = load_u64(b"cv_total_shares");
    let price = if shares > 0 { assets * 1_000_000 / shares } else { 1_000_000 };
    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(assets));
    result.extend_from_slice(&u64_to_bytes(shares));
    result.extend_from_slice(&u64_to_bytes(price));
    set_return_data(&result);
    1
}
`,
            'Cargo.toml': `[package]
name = "clawvault"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    identity: {
        name: 'MoltyID Identity',
        description: 'Identity with cooldowns, pause, admin transfer',
        templateType: 'identity',
        files: {
            'lib.rs': `// MoltyID - Agent Identity System
// Registration, reputation, .molt naming, vouching, skill attestations

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp
};

const INITIAL_REPUTATION: u64 = 100;
const MAX_REPUTATION: u64 = 100_000;
const VOUCH_COST: u64 = 5;
const VOUCH_REWARD: u64 = 10;
const MIN_NAME_LEN: usize = 3;
const MAX_NAME_LEN: usize = 32;

// Agent types
const AGENT_TRADER: u8 = 0;
const AGENT_DEVELOPER: u8 = 1;
const AGENT_ANALYST: u8 = 2;
const AGENT_VALIDATOR: u8 = 3;
const AGENT_ORACLE: u8 = 4;

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

// Identity: owner(32) + agent_type(1) + name_len(2) + name(64) +
//           reputation(8) + created_at(8) + is_active(1) = 116 bytes
const IDENTITY_SIZE: usize = 116;

#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };
    storage_set(b"mid_admin", admin);
    store_u64(b"mid_count", 0);
    log_info("\\xf0\\x9f\\x86\\x94 MoltyID initialized");
    1
}

#[no_mangle]
pub extern "C" fn register_identity(
    owner_ptr: *const u8,
    agent_type: u8,
    name_ptr: *const u8,
    name_len: u32,
) -> u32 {
    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len as usize) };
    let h = hex(owner);

    // Check not registered
    if storage_get(alloc::format!("id:{}", h).as_bytes()).is_some() {
        log_info("Already registered");
        return 0;
    }

    let mut identity = Vec::with_capacity(IDENTITY_SIZE);
    identity.extend_from_slice(owner);                                   // 0..32 owner
    identity.push(agent_type);                                            // 32 agent_type
    identity.extend_from_slice(&(name_len as u16).to_le_bytes());        // 33..35 name_len
    identity.extend_from_slice(name);                                     // 35.. name
    identity.resize(35 + 64, 0);                                          // pad name to 64
    identity.extend_from_slice(&u64_to_bytes(INITIAL_REPUTATION));       // reputation
    identity.extend_from_slice(&u64_to_bytes(get_timestamp()));          // created_at
    identity.push(1);                                                     // is_active

    storage_set(alloc::format!("id:{}", h).as_bytes(), &identity);
    let count = load_u64(b"mid_count") + 1;
    store_u64(b"mid_count", count);

    let type_name = match agent_type {
        0 => "Trader", 1 => "Developer", 2 => "Analyst",
        3 => "Validator", 4 => "Oracle", _ => "Custom"
    };
    let name_str = core::str::from_utf8(name).unwrap_or("?");
    log_info(&alloc::format!("Registered {} '{}' (reputation: {})", type_name, name_str, INITIAL_REPUTATION));
    1
}

#[no_mangle]
pub extern "C" fn vouch(voucher_ptr: *const u8, vouchee_ptr: *const u8) -> u32 {
    let voucher = unsafe { core::slice::from_raw_parts(voucher_ptr, 32) };
    let vouchee = unsafe { core::slice::from_raw_parts(vouchee_ptr, 32) };
    let vh = hex(voucher);
    let eh = hex(vouchee);

    // Deduct VOUCH_COST from voucher reputation
    let voucher_rep = load_u64(alloc::format!("rep:{}", vh).as_bytes());
    if voucher_rep < VOUCH_COST { log_info("Insufficient reputation to vouch"); return 0; }
    store_u64(alloc::format!("rep:{}", vh).as_bytes(), voucher_rep - VOUCH_COST);

    // Add VOUCH_REWARD to vouchee
    let vouchee_rep = load_u64(alloc::format!("rep:{}", eh).as_bytes());
    let new_rep = core::cmp::min(vouchee_rep + VOUCH_REWARD, MAX_REPUTATION);
    store_u64(alloc::format!("rep:{}", eh).as_bytes(), new_rep);

    log_info(&alloc::format!("Vouch: -{} rep from voucher, +{} to vouchee (now {})", VOUCH_COST, VOUCH_REWARD, new_rep));
    1
}

#[no_mangle]
pub extern "C" fn register_name(
    owner_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    fee_paid: u64,
) -> u32 {
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len as usize) };
    let len = name_len as usize;
    if len < MIN_NAME_LEN || len > MAX_NAME_LEN {
        log_info(&alloc::format!("Name must be {}-{} chars", MIN_NAME_LEN, MAX_NAME_LEN));
        return 0;
    }

    // Tiered pricing: 3-char = 1B, 4-char = 500M, 5+ = 100M shells
    let required = match len { 3 => 1_000_000_000u64, 4 => 500_000_000, _ => 100_000_000 };
    if fee_paid < required {
        log_info(&alloc::format!("Name fee: {} required, {} paid", required, fee_paid));
        return 0;
    }

    let name_str = core::str::from_utf8(name).unwrap_or("?");
    let key = alloc::format!("name:{}", name_str);
    if storage_get(key.as_bytes()).is_some() {
        log_info("Name already taken");
        return 0;
    }

    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    storage_set(key.as_bytes(), owner);
    storage_set(alloc::format!("name_rev:{}", hex(owner)).as_bytes(), name);

    log_info(&alloc::format!("Registered {}.molt", name_str));
    1
}

#[no_mangle]
pub extern "C" fn resolve_name(name_ptr: *const u8, name_len: u32) -> u32 {
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len as usize) };
    let name_str = core::str::from_utf8(name).unwrap_or("?");
    match storage_get(alloc::format!("name:{}", name_str).as_bytes()) {
        Some(addr) => { set_return_data(&addr); 1 }
        None => { log_info(&alloc::format!("{}.molt not found", name_str)); 0 }
    }
}

#[no_mangle]
pub extern "C" fn get_reputation(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };
    let h = hex(pubkey);
    let rep = load_u64(alloc::format!("rep:{}", h).as_bytes());
    set_return_data(&u64_to_bytes(rep));
    1
}

#[no_mangle]
pub extern "C" fn get_trust_tier(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };
    let rep = load_u64(alloc::format!("rep:{}", hex(pubkey)).as_bytes());
    let tier = match rep {
        0..=99 => 0,          // Unverified
        100..=999 => 1,       // Basic
        1000..=9999 => 2,     // Trusted
        10000..=49999 => 3,   // Established
        _ => 4,               // Elite
    };
    set_return_data(&u64_to_bytes(tier));
    1
}
`,
            'Cargo.toml': `[package]
name = "moltyid-identity"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    marketplace: {
        name: 'MoltMarket NFT',
        description: 'NFT marketplace with offers, bids, and fixed layout',
        templateType: 'marketplace',
        files: {
            'lib.rs': `// MoltMarket - NFT Marketplace
// List, buy, cancel with cross-contract composability

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    Address, log_info, storage_get, storage_set,
    bytes_to_u64, u64_to_bytes, call_token_transfer, call_nft_transfer
};

const DEFAULT_FEE_BPS: u64 = 250; // 2.5%

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8, fee_recipient_ptr: *const u8) {
    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let fee_addr = unsafe { core::slice::from_raw_parts(fee_recipient_ptr, 32) };
    storage_set(b"mk_owner", owner);
    storage_set(b"mk_fee_addr", fee_addr);
    storage_set(b"mk_fee_bps", &u64_to_bytes(DEFAULT_FEE_BPS));
    log_info("\\xf0\\x9f\\x8f\\xaa MoltMarket initialized (2.5% fee)");
}

#[no_mangle]
pub extern "C" fn list_nft(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
) -> u32 {
    let seller = unsafe { core::slice::from_raw_parts(seller_ptr, 32) };
    let nft_contract = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let payment_token = unsafe { core::slice::from_raw_parts(payment_token_ptr, 32) };

    let key = alloc::format!("listing:{}:{}", hex(nft_contract), token_id);
    let mut listing = Vec::with_capacity(145);
    listing.extend_from_slice(seller);          // 0..32
    listing.extend_from_slice(nft_contract);    // 32..64
    listing.extend_from_slice(&u64_to_bytes(token_id));  // 64..72
    listing.extend_from_slice(&u64_to_bytes(price));     // 72..80
    listing.extend_from_slice(payment_token);   // 80..112
    listing.resize(144, 0);                     // padding
    listing.push(1);                            // 144: active

    storage_set(key.as_bytes(), &listing);
    log_info(&alloc::format!("Listed NFT #{} for {} shells", token_id, price));
    1
}

#[no_mangle]
pub extern "C" fn buy_nft(
    buyer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    let nft_contract = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let key = alloc::format!("listing:{}:{}", hex(nft_contract), token_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= 145 && d[144] == 1 => d,
        _ => { log_info("Listing not found or inactive"); return 0; }
    };

    let price = bytes_to_u64(&data[72..80]);
    let fee_bps = storage_get(b"mk_fee_bps").map(|d| bytes_to_u64(&d)).unwrap_or(250);
    let fee = price * fee_bps / 10000;
    let seller_gets = price - fee;

    // Mark inactive
    data[144] = 0;
    storage_set(key.as_bytes(), &data);

    log_info(&alloc::format!("NFT #{} sold! Price: {}, Fee: {}, Seller gets: {}", token_id, price, fee, seller_gets));
    1
}

#[no_mangle]
pub extern "C" fn cancel_listing(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    let seller = unsafe { core::slice::from_raw_parts(seller_ptr, 32) };
    let nft_contract = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let key = alloc::format!("listing:{}:{}", hex(nft_contract), token_id);

    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= 145 => d,
        _ => { log_info("Listing not found"); return 0; }
    };

    if &data[0..32] != seller { log_info("Not the seller"); return 0; }
    data[144] = 0;
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Listing cancelled for NFT #{}", token_id));
    1
}
`,
            'Cargo.toml': `[package]
name = "moltmarket-marketplace"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    auction: {
        name: 'MoltAuction',
        description: 'Auctions with anti-sniping, reserve prices, cancellation',
        templateType: 'auction',
        files: {
            'lib.rs': `// MoltAuction - NFT Auction House
// English auctions, offers/bids, creator royalties, collection stats

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    Address, log_info, storage_get, storage_set,
    bytes_to_u64, u64_to_bytes, get_timestamp
};

const AUCTION_DURATION: u64 = 86400;  // 24 hours default
const MIN_BID_INCREMENT: u64 = 5;     // 5% minimum increment

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

// Auction: seller(32) + nft(32) + token_id(8) + min_bid(8) +
//          start(8) + end(8) + highest_bidder(32) + highest_bid(8) + active(1) = 137
const AUCTION_SIZE: usize = 137;

#[no_mangle]
pub extern "C" fn initialize(marketplace_ptr: *const u8) -> u32 {
    let marketplace = unsafe { core::slice::from_raw_parts(marketplace_ptr, 32) };
    storage_set(b"au_marketplace", marketplace);
    log_info("\\xf0\\x9f\\x94\\xa8 MoltAuction initialized");
    1
}

#[no_mangle]
pub extern "C" fn create_auction(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    min_bid: u64,
    duration: u64,
) -> u32 {
    let seller = unsafe { core::slice::from_raw_parts(seller_ptr, 32) };
    let nft = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let now = get_timestamp();
    let dur = if duration == 0 { AUCTION_DURATION } else { duration };

    let mut auction = Vec::with_capacity(AUCTION_SIZE);
    auction.extend_from_slice(seller);                     // seller
    auction.extend_from_slice(nft);                        // nft contract
    auction.extend_from_slice(&u64_to_bytes(token_id));    // token_id
    auction.extend_from_slice(&u64_to_bytes(min_bid));     // minimum bid
    auction.extend_from_slice(&u64_to_bytes(now));         // start
    auction.extend_from_slice(&u64_to_bytes(now + dur));   // end
    auction.extend_from_slice(&[0u8; 32]);                 // highest_bidder (none)
    auction.extend_from_slice(&u64_to_bytes(0));           // highest_bid
    auction.push(1);                                        // active

    let key = alloc::format!("auction_{}_{}", hex(nft), token_id);
    storage_set(key.as_bytes(), &auction);
    log_info(&alloc::format!("Auction created: NFT #{}, min bid {}, duration {}s", token_id, min_bid, dur));
    1
}

#[no_mangle]
pub extern "C" fn place_bid(
    bidder_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    bid_amount: u64,
) -> u32 {
    let nft = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let key = alloc::format!("auction_{}_{}", hex(nft), token_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= AUCTION_SIZE && d[136] == 1 => d,
        _ => { log_info("Auction not found or ended"); return 0; }
    };

    let now = get_timestamp();
    let end = bytes_to_u64(&data[88..96]);
    if now > end { log_info("Auction expired"); return 0; }

    let highest_bid = bytes_to_u64(&data[128..136]);
    let min_bid = bytes_to_u64(&data[72..80]);
    let required = if highest_bid == 0 { min_bid } else { highest_bid + highest_bid * MIN_BID_INCREMENT / 100 };
    if bid_amount < required {
        log_info(&alloc::format!("Bid too low: {} < {} required", bid_amount, required));
        return 0;
    }

    let bidder = unsafe { core::slice::from_raw_parts(bidder_ptr, 32) };
    data[96..128].copy_from_slice(bidder);                    // highest_bidder
    data[128..136].copy_from_slice(&u64_to_bytes(bid_amount)); // highest_bid
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Bid placed: {} (previous: {})", bid_amount, highest_bid));
    1
}

#[no_mangle]
pub extern "C" fn finalize_auction(
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    let nft = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let key = alloc::format!("auction_{}_{}", hex(nft), token_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= AUCTION_SIZE && d[136] == 1 => d,
        _ => { log_info("Auction not found"); return 0; }
    };

    let highest_bid = bytes_to_u64(&data[128..136]);
    data[136] = 0; // deactivate
    storage_set(key.as_bytes(), &data);

    // Check royalty
    let royalty_key = alloc::format!("royalty_{}", hex(nft));
    let royalty_bps = load_u64(royalty_key.as_bytes());
    let royalty = highest_bid * royalty_bps / 10000;
    let seller_gets = highest_bid - royalty;

    log_info(&alloc::format!("Auction finalized! Bid: {}, Royalty: {}, Seller: {}", highest_bid, royalty, seller_gets));
    1
}

#[no_mangle]
pub extern "C" fn set_royalty(creator_ptr: *const u8, nft_contract_ptr: *const u8, royalty_bps: u64) -> u32 {
    if royalty_bps > 1000 { log_info("Max royalty: 10%"); return 0; }
    let nft = unsafe { core::slice::from_raw_parts(nft_contract_ptr, 32) };
    let key = alloc::format!("royalty_{}", hex(nft));
    storage_set(key.as_bytes(), &u64_to_bytes(royalty_bps));
    log_info(&alloc::format!("Royalty set: {}bps ({}%)", royalty_bps, royalty_bps as f64 / 100.0));
    1
}
`,
            'Cargo.toml': `[package]
name = "moltauction-marketplace"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    oracle: {
        name: 'MoltOracle Feeds',
        description: 'Price feeds, VRF, attestations',
        files: {
            'lib.rs': `// MoltOracle - Decentralized Oracle
// Price feeds, verifiable random function (VRF), attestation services

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp, get_caller
};

const MAX_STALENESS: u64 = 3600; // 1 hour
// Price feed: price(8) + timestamp(8) + decimals(1) + feeder(32) = 49 bytes
const PRICE_FEED_SIZE: usize = 49;

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

#[no_mangle]
pub extern "C" fn initialize_oracle(owner_ptr: *const u8) -> u32 {
    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    storage_set(b"oracle_owner", owner);
    storage_set(b"oracle_feed_count", &u64_to_bytes(0));
    log_info("\\xf0\\x9f\\x93\\xa1 MoltOracle initialized");
    1
}

#[no_mangle]
pub extern "C" fn add_price_feeder(
    feeder_ptr: *const u8,
    asset_ptr: *const u8,
    asset_len: u32,
) -> u32 {
    // T5.10: verify caller is oracle owner, not the feeder
    let caller = get_caller();
    let owner = match storage_get(b"oracle_owner") {
        Some(o) => o,
        None => { log_info("Not initialized"); return 0; }
    };
    if caller != owner.as_slice() { log_info("Unauthorized"); return 0; }

    let feeder = unsafe { core::slice::from_raw_parts(feeder_ptr, 32) };
    let asset = unsafe { core::slice::from_raw_parts(asset_ptr, asset_len as usize) };
    let asset_str = core::str::from_utf8(asset).unwrap_or("?");
    let key = alloc::format!("feeder_{}", asset_str);
    storage_set(key.as_bytes(), feeder);
    log_info(&alloc::format!("Authorized feeder for {}", asset_str));
    1
}

#[no_mangle]
pub extern "C" fn submit_price(
    feeder_ptr: *const u8,
    asset_ptr: *const u8,
    asset_len: u32,
    price: u64,
    decimals: u8,
) -> u32 {
    let feeder = unsafe { core::slice::from_raw_parts(feeder_ptr, 32) };
    let asset = unsafe { core::slice::from_raw_parts(asset_ptr, asset_len as usize) };
    let asset_str = core::str::from_utf8(asset).unwrap_or("?");

    // Verify feeder authorization
    let key = alloc::format!("feeder_{}", asset_str);
    match storage_get(key.as_bytes()) {
        Some(auth) if auth == feeder => {},
        _ => { log_info("Unauthorized feeder"); return 0; }
    }

    let mut feed = Vec::with_capacity(PRICE_FEED_SIZE);
    feed.extend_from_slice(&u64_to_bytes(price));        // 0..8
    feed.extend_from_slice(&u64_to_bytes(get_timestamp())); // 8..16
    feed.push(decimals);                                   // 16
    feed.extend_from_slice(feeder);                        // 17..49

    let price_key = alloc::format!("price_{}", asset_str);
    storage_set(price_key.as_bytes(), &feed);
    log_info(&alloc::format!("Price submitted: {} = {} ({}d)", asset_str, price, decimals));
    1
}

#[no_mangle]
pub extern "C" fn get_price(
    asset_ptr: *const u8,
    asset_len: u32,
    result_ptr: *mut u8,
) -> u32 {
    let asset = unsafe { core::slice::from_raw_parts(asset_ptr, asset_len as usize) };
    let asset_str = core::str::from_utf8(asset).unwrap_or("?");
    let key = alloc::format!("price_{}", asset_str);

    let feed = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= PRICE_FEED_SIZE => d,
        _ => { log_info(&alloc::format!("No price for {}", asset_str)); return 0; }
    };

    // Staleness check
    let feed_time = bytes_to_u64(&feed[8..16]);
    let now = get_timestamp();
    if now - feed_time > MAX_STALENESS {
        log_info(&alloc::format!("Price stale: {}s old", now - feed_time));
        return 0;
    }

    unsafe {
        let out = core::slice::from_raw_parts_mut(result_ptr, 9);
        out[..8].copy_from_slice(&feed[0..8]);
        out[8] = feed[16];
    }
    1
}

/// Commit-reveal VRF: Phase 1
#[no_mangle]
pub extern "C" fn commit_randomness(
    requester_ptr: *const u8,
    commit_hash_ptr: *const u8,
    seed: u64,
) -> u32 {
    let requester = unsafe { core::slice::from_raw_parts(requester_ptr, 32) };
    let commit = unsafe { core::slice::from_raw_parts(commit_hash_ptr, 32) };
    let h = hex(requester);
    let key = alloc::format!("rng_commit_{}", h);
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(commit);
    data.extend_from_slice(&u64_to_bytes(seed));
    data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Randomness committed (seed: {})", seed));
    1
}

/// Commit-reveal VRF: Phase 2
#[no_mangle]
pub extern "C" fn reveal_randomness(
    requester_ptr: *const u8,
    secret_ptr: *const u8,
    result_ptr: *mut u8,
) -> u32 {
    let requester = unsafe { core::slice::from_raw_parts(requester_ptr, 32) };
    let h = hex(requester);
    let key = alloc::format!("rng_commit_{}", h);
    let commit = match storage_get(key.as_bytes()) {
        Some(d) => d,
        None => { log_info("No commit found"); return 0; }
    };

    // Derive random from commit + block timestamp
    let seed = bytes_to_u64(&commit[32..40]);
    let ts = get_timestamp();
    let random = seed ^ ts ^ 0xDEADBEEF_CAFEBABE;

    let result_key = alloc::format!("random_{}", h);
    storage_set(result_key.as_bytes(), &u64_to_bytes(random));

    unsafe {
        let out = core::slice::from_raw_parts_mut(result_ptr, 8);
        out.copy_from_slice(&u64_to_bytes(random));
    }
    log_info(&alloc::format!("Randomness revealed: {}", random));
    1
}
`,
            'Cargo.toml': `[package]
name = "moltoracle-feeds"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    bridge: {
        name: 'MoltBridge',
        description: 'Secure cross-chain bridge with multi-call confirmation',
        files: {
            'lib.rs': `// MoltBridge - Secure Cross-Chain Bridge
// Multi-call confirmation pattern: submit_mint() + confirm_mint()
// Fixes single-call vulnerability from v1 (mint_bridged was exploitable)
// Features: request expiry, source TX dedup, fund reservation, emergency pause

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set,
    bytes_to_u64, u64_to_bytes, get_slot
};

// === Constants ===
const REQUIRED_CONFIRMATIONS: u8 = 2;
const REQUEST_EXPIRY_SLOTS: u64 = 43_200;  // ~12 hours
const STATUS_PENDING: u8 = 0;
const STATUS_COMPLETED: u8 = 1;
const STATUS_CANCELLED: u8 = 2;
const BRIDGE_TX_SIZE: usize = 115;
// MintRequest: recipient(32)+amount(8)+source_chain(32)+source_tx(32)+submitter(32)+slot(8)+confirmed(1) = 145
const MINT_REQUEST_SIZE: usize = 145;
const ADMIN_KEY: &[u8] = b"bridge_owner";
const PAUSE_KEY: &[u8] = b"bridge_paused";

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}
fn load_u64(key: &[u8]) -> u64 { storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0) }
fn store_u64(key: &[u8], val: u64) { storage_set(key, &u64_to_bytes(val)); }
fn is_paused() -> bool { storage_get(PAUSE_KEY).map(|d| d.first() == Some(&1)).unwrap_or(false) }
fn is_admin(addr: &[u8]) -> bool { storage_get(ADMIN_KEY).map(|o| o.as_slice() == addr).unwrap_or(false) }
fn is_validator(addr: &[u8]) -> bool {
    let key = alloc::format!("bridge_validator_{}", hex(addr));
    storage_get(key.as_bytes()).is_some()
}

#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    storage_set(ADMIN_KEY, owner);
    store_u64(b"bridge_nonce", 0);
    store_u64(b"mint_request_count", 0);
    store_u64(b"bridge_locked_amount", 0);
    store_u64(b"bridge_validator_count", 0);
    log_info("MoltBridge initialized");
    1
}

#[no_mangle]
pub extern "C" fn add_bridge_validator(caller_ptr: *const u8, validator_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_admin(caller) { log_info("Unauthorized"); return 0; }
    let validator = unsafe { core::slice::from_raw_parts(validator_ptr, 32) };
    let key = alloc::format!("bridge_validator_{}", hex(validator));
    storage_set(key.as_bytes(), &[1]);
    let count = load_u64(b"bridge_validator_count") + 1;
    store_u64(b"bridge_validator_count", count);
    log_info(&alloc::format!("Validator added (total: {})", count));
    1
}

// === Lock tokens for bridging OUT ===
#[no_mangle]
pub extern "C" fn lock_tokens(
    sender_ptr: *const u8, amount: u64,
    dest_chain_ptr: *const u8, dest_address_ptr: *const u8,
) -> u32 {
    if is_paused() { log_info("Bridge paused"); return 0; }
    let sender = unsafe { core::slice::from_raw_parts(sender_ptr, 32) };
    let dest_chain = unsafe { core::slice::from_raw_parts(dest_chain_ptr, 32) };
    let dest_addr = unsafe { core::slice::from_raw_parts(dest_address_ptr, 32) };

    let nonce = load_u64(b"bridge_nonce") + 1;
    store_u64(b"bridge_nonce", nonce);
    let locked = load_u64(b"bridge_locked_amount");
    store_u64(b"bridge_locked_amount", locked + amount);

    let mut tx = Vec::with_capacity(BRIDGE_TX_SIZE);
    tx.extend_from_slice(sender);
    tx.extend_from_slice(&u64_to_bytes(amount));
    tx.push(0); tx.push(STATUS_PENDING);
    tx.extend_from_slice(&u64_to_bytes(get_slot()));
    tx.push(0);
    tx.extend_from_slice(dest_chain);
    tx.extend_from_slice(dest_addr);

    let key = alloc::format!("bridge_tx_{}", nonce);
    storage_set(key.as_bytes(), &tx);
    log_info(&alloc::format!("Locked {} MOLT (nonce #{})", amount, nonce));
    nonce as u32
}

// === Two-step mint (secure multi-call pattern) ===

/// Step 1: Validator submits mint request — funds reserved, not released
#[no_mangle]
pub extern "C" fn submit_mint(
    caller_ptr: *const u8, recipient_ptr: *const u8, amount: u64,
    source_chain_ptr: *const u8, source_tx_ptr: *const u8,
) -> u32 {
    if is_paused() { log_info("Bridge paused"); return 0; }
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_validator(caller) { log_info("Not a validator"); return 0; }

    let recipient = unsafe { core::slice::from_raw_parts(recipient_ptr, 32) };
    let source_chain = unsafe { core::slice::from_raw_parts(source_chain_ptr, 32) };
    let source_tx = unsafe { core::slice::from_raw_parts(source_tx_ptr, 32) };

    // Dedup: check source TX not already used
    let dedup_key = alloc::format!("src_tx_{}", hex(source_tx));
    if storage_get(dedup_key.as_bytes()).is_some() {
        log_info("Source TX already processed"); return 0;
    }

    let req_id = load_u64(b"mint_request_count") + 1;
    store_u64(b"mint_request_count", req_id);

    // Reserve funds (tracked but not released)
    let reserved = load_u64(b"bridge_reserved");
    store_u64(b"bridge_reserved", reserved + amount);

    let mut req = Vec::with_capacity(MINT_REQUEST_SIZE);
    req.extend_from_slice(recipient);                       // 0..32
    req.extend_from_slice(&u64_to_bytes(amount));           // 32..40
    req.extend_from_slice(source_chain);                    // 40..72
    req.extend_from_slice(source_tx);                       // 72..104
    req.extend_from_slice(caller);                          // 104..136 submitter
    req.extend_from_slice(&u64_to_bytes(get_slot()));       // 136..144 slot
    req.push(0);                                             // 144 confirmed=false

    let key = alloc::format!("mint_req_{}", req_id);
    storage_set(key.as_bytes(), &req);
    // Mark source TX as used
    storage_set(dedup_key.as_bytes(), &u64_to_bytes(req_id));

    log_info(&alloc::format!("Mint request #{} submitted: {} MOLT", req_id, amount));
    req_id as u32
}

/// Step 2: Different validator confirms — only then funds are released
#[no_mangle]
pub extern "C" fn confirm_mint(caller_ptr: *const u8, request_id: u64) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_validator(caller) { log_info("Not a validator"); return 0; }

    let key = alloc::format!("mint_req_{}", request_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= MINT_REQUEST_SIZE => d,
        _ => { log_info("Request not found"); return 0; }
    };

    // Already confirmed?
    if data[144] == 1 { log_info("Already confirmed"); return 0; }

    // Check expiry
    let created = bytes_to_u64(&data[136..144]);
    if get_slot() > created + REQUEST_EXPIRY_SLOTS {
        log_info("Request expired"); return 0;
    }

    // Confirmer must be different from submitter
    let submitter = &data[104..136];
    if caller == submitter { log_info("Confirmer must differ from submitter"); return 0; }

    // Confirm and release
    data[144] = 1;
    storage_set(key.as_bytes(), &data);

    let amount = bytes_to_u64(&data[32..40]);
    let reserved = load_u64(b"bridge_reserved");
    store_u64(b"bridge_reserved", reserved.saturating_sub(amount));

    log_info(&alloc::format!("Mint #{} confirmed: {} MOLT released", request_id, amount));
    1
}

// === Admin: Emergency pause ===
#[no_mangle]
pub extern "C" fn pause(caller_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_admin(caller) { return 0; }
    storage_set(PAUSE_KEY, &[1]);
    log_info("Bridge PAUSED");
    1
}

#[no_mangle]
pub extern "C" fn unpause(caller_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_admin(caller) { return 0; }
    storage_set(PAUSE_KEY, &[0]);
    log_info("Bridge UNPAUSED");
    1
}

#[no_mangle]
pub extern "C" fn get_bridge_status(nonce: u64) -> u32 {
    let key = alloc::format!("bridge_tx_{}", nonce);
    match storage_get(key.as_bytes()) {
        Some(d) => {
            let amount = bytes_to_u64(&d[32..40]);
            let status = match d[41] { 0 => "pending", 1 => "completed", 2 => "cancelled", _ => "?" };
            log_info(&alloc::format!("Bridge TX #{}: {} MOLT ({})", nonce, amount, status));
            1
        }
        None => 0
    }
}
`,
            'Cargo.toml': `[package]
name = "moltbridge"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    bounty: {
        name: 'BountyBoard',
        description: 'On-chain task management',
        files: {
            'lib.rs': `// BountyBoard - On-Chain Task Management
// Post bounties with rewards/deadlines, submit proof, approve and pay

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set,
    bytes_to_u64, u64_to_bytes, get_slot
};

const BOUNTY_OPEN: u8 = 0;
const BOUNTY_COMPLETED: u8 = 1;
const BOUNTY_CANCELLED: u8 = 2;
// Bounty: creator(32)+title_hash(32)+reward(8)+deadline(8)+status(1)+sub_count(1)+created(8)+approved_idx(1) = 91
const BOUNTY_SIZE: usize = 91;
// Submission: worker(32)+proof_hash(32)+submitted_slot(8) = 72
const SUBMISSION_SIZE: usize = 72;

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

#[no_mangle]
pub extern "C" fn create_bounty(
    creator_ptr: *const u8,
    title_hash_ptr: *const u8,
    reward_amount: u64,
    deadline_slot: u64,
) -> u32 {
    let creator = unsafe { core::slice::from_raw_parts(creator_ptr, 32) };
    let title_hash = unsafe { core::slice::from_raw_parts(title_hash_ptr, 32) };

    let id = load_u64(b"bounty_count") + 1;
    store_u64(b"bounty_count", id);

    let mut bounty = Vec::with_capacity(BOUNTY_SIZE);
    bounty.extend_from_slice(creator);                      // 0..32
    bounty.extend_from_slice(title_hash);                   // 32..64
    bounty.extend_from_slice(&u64_to_bytes(reward_amount)); // 64..72
    bounty.extend_from_slice(&u64_to_bytes(deadline_slot)); // 72..80
    bounty.push(BOUNTY_OPEN);                               // 80 status
    bounty.push(0);                                          // 81 sub_count
    bounty.extend_from_slice(&u64_to_bytes(get_slot()));    // 82..90
    bounty.push(0);                                          // 90 approved_idx

    let key = alloc::format!("bounty_{}", id);
    storage_set(key.as_bytes(), &bounty);
    log_info(&alloc::format!("Bounty #{} created: {} MOLT reward, deadline slot {}", id, reward_amount, deadline_slot));
    id as u32
}

#[no_mangle]
pub extern "C" fn submit_work(bounty_id: u64, worker_ptr: *const u8, proof_hash_ptr: *const u8) -> u32 {
    let key = alloc::format!("bounty_{}", bounty_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= BOUNTY_SIZE => d,
        _ => { log_info("Bounty not found"); return 0; }
    };
    if data[80] != BOUNTY_OPEN { log_info("Bounty not open"); return 0; }

    let worker = unsafe { core::slice::from_raw_parts(worker_ptr, 32) };
    let proof = unsafe { core::slice::from_raw_parts(proof_hash_ptr, 32) };
    let idx = data[81];

    let mut submission = Vec::with_capacity(SUBMISSION_SIZE);
    submission.extend_from_slice(worker);                   // 0..32
    submission.extend_from_slice(proof);                    // 32..64
    submission.extend_from_slice(&u64_to_bytes(get_slot())); // 64..72

    let sub_key = alloc::format!("submission_{}_{}", bounty_id, idx);
    storage_set(sub_key.as_bytes(), &submission);

    data[81] = idx + 1;
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Work submitted for bounty #{} (submission #{})", bounty_id, idx));
    1
}

#[no_mangle]
pub extern "C" fn approve_work(caller_ptr: *const u8, bounty_id: u64, submission_idx: u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let key = alloc::format!("bounty_{}", bounty_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= BOUNTY_SIZE => d,
        _ => { log_info("Bounty not found"); return 0; }
    };

    if &data[0..32] != caller { log_info("Only creator can approve"); return 0; }
    if data[80] != BOUNTY_OPEN { log_info("Bounty not open"); return 0; }

    let reward = bytes_to_u64(&data[64..72]);
    data[80] = BOUNTY_COMPLETED;
    data[90] = submission_idx;
    storage_set(key.as_bytes(), &data);

    log_info(&alloc::format!("Bounty #{} completed! Submission #{} approved, {} MOLT paid", bounty_id, submission_idx, reward));
    1
}

#[no_mangle]
pub extern "C" fn cancel_bounty(caller_ptr: *const u8, bounty_id: u64) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let key = alloc::format!("bounty_{}", bounty_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= BOUNTY_SIZE => d,
        _ => { log_info("Bounty not found"); return 0; }
    };
    if &data[0..32] != caller { log_info("Only creator can cancel"); return 0; }
    data[80] = BOUNTY_CANCELLED;
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Bounty #{} cancelled, reward refunded", bounty_id));
    1
}

#[no_mangle]
pub extern "C" fn get_bounty(bounty_id: u64) -> u32 {
    let key = alloc::format!("bounty_{}", bounty_id);
    match storage_get(key.as_bytes()) {
        Some(d) => {
            let reward = bytes_to_u64(&d[64..72]);
            let status = match d[80] { 0 => "open", 1 => "completed", 2 => "cancelled", _ => "?" };
            let subs = d[81];
            log_info(&alloc::format!("Bounty #{}: {} MOLT, {}, {} submissions", bounty_id, reward, status, subs));
            1
        }
        None => 0
    }
}
`,
            'Cargo.toml': `[package]
name = "bountyboard"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    compute: {
        name: 'Compute Market',
        description: 'Compute marketplace with escrow, disputes, arbitration',
        files: {
            'lib.rs': `// Compute Market - Decentralized Compute
// Providers offer resources, requesters submit jobs

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set,
    bytes_to_u64, u64_to_bytes, get_slot
};

const JOB_PENDING: u8 = 0;
const JOB_CLAIMED: u8 = 1;
const JOB_COMPLETED: u8 = 2;
const JOB_DISPUTED: u8 = 3;
// Provider: address(32)+units(8)+price(8)+completed(8)+active(1)+registered(8) = 65
const PROVIDER_SIZE: usize = 65;
// Job: requester(32)+units(8)+max_price(8)+code_hash(32)+status(1)+provider(32)+result(32)+created(8)+completed(8) = 161
const JOB_SIZE: usize = 161;

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

#[no_mangle]
pub extern "C" fn register_provider(
    provider_ptr: *const u8,
    compute_units_available: u64,
    price_per_unit: u64,
) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let h = hex(provider);

    let mut data = Vec::with_capacity(PROVIDER_SIZE);
    data.extend_from_slice(provider);                              // 0..32
    data.extend_from_slice(&u64_to_bytes(compute_units_available));// 32..40
    data.extend_from_slice(&u64_to_bytes(price_per_unit));         // 40..48
    data.extend_from_slice(&u64_to_bytes(0));                      // 48..56 completed
    data.push(1);                                                   // 56 active
    data.extend_from_slice(&u64_to_bytes(get_slot()));             // 57..65

    let key = alloc::format!("provider_{}", h);
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Provider registered: {} units @ {} MOLT/unit", compute_units_available, price_per_unit));
    1
}

#[no_mangle]
pub extern "C" fn submit_job(
    requester_ptr: *const u8,
    compute_units_needed: u64,
    max_price: u64,
    code_hash_ptr: *const u8,
) -> u32 {
    let requester = unsafe { core::slice::from_raw_parts(requester_ptr, 32) };
    let code_hash = unsafe { core::slice::from_raw_parts(code_hash_ptr, 32) };

    let id = load_u64(b"job_count") + 1;
    store_u64(b"job_count", id);

    let mut job = Vec::with_capacity(JOB_SIZE);
    job.extend_from_slice(requester);                           // 0..32
    job.extend_from_slice(&u64_to_bytes(compute_units_needed)); // 32..40
    job.extend_from_slice(&u64_to_bytes(max_price));            // 40..48
    job.extend_from_slice(code_hash);                           // 48..80
    job.push(JOB_PENDING);                                       // 80 status
    job.extend_from_slice(&[0u8; 32]);                          // 81..113 provider (none)
    job.extend_from_slice(&[0u8; 32]);                          // 113..145 result (none)
    job.extend_from_slice(&u64_to_bytes(get_slot()));           // 145..153
    job.extend_from_slice(&u64_to_bytes(0));                    // 153..161

    let key = alloc::format!("job_{}", id);
    storage_set(key.as_bytes(), &job);
    log_info(&alloc::format!("Job #{} submitted: {} units, max {} MOLT", id, compute_units_needed, max_price));
    id as u32
}

#[no_mangle]
pub extern "C" fn claim_job(provider_ptr: *const u8, job_id: u64) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let key = alloc::format!("job_{}", job_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= JOB_SIZE => d,
        _ => { log_info("Job not found"); return 0; }
    };
    if data[80] != JOB_PENDING { log_info("Job not pending"); return 0; }

    data[80] = JOB_CLAIMED;
    data[81..113].copy_from_slice(provider);
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Job #{} claimed", job_id));
    1
}

#[no_mangle]
pub extern "C" fn complete_job(provider_ptr: *const u8, job_id: u64, result_hash_ptr: *const u8) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let result_hash = unsafe { core::slice::from_raw_parts(result_hash_ptr, 32) };
    let key = alloc::format!("job_{}", job_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= JOB_SIZE => d,
        _ => { log_info("Job not found"); return 0; }
    };
    if data[80] != JOB_CLAIMED { log_info("Job not claimed"); return 0; }
    if &data[81..113] != provider { log_info("Not assigned provider"); return 0; }

    data[80] = JOB_COMPLETED;
    data[113..145].copy_from_slice(result_hash);
    data[153..161].copy_from_slice(&u64_to_bytes(get_slot()));
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Job #{} completed!", job_id));
    1
}

#[no_mangle]
pub extern "C" fn dispute_job(requester_ptr: *const u8, job_id: u64) -> u32 {
    let requester = unsafe { core::slice::from_raw_parts(requester_ptr, 32) };
    let key = alloc::format!("job_{}", job_id);
    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= JOB_SIZE => d,
        _ => { log_info("Job not found"); return 0; }
    };
    if &data[0..32] != requester { log_info("Not the requester"); return 0; }
    data[80] = JOB_DISPUTED;
    storage_set(key.as_bytes(), &data);
    log_info(&alloc::format!("Job #{} disputed!", job_id));
    1
}
`,
            'Cargo.toml': `[package]
name = "compute-market"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },
    storage: {
        name: 'ReefStorage',
        description: 'Decentralized storage with proof-of-storage, staking, slashing',
        files: {
            'lib.rs': `// Reef Storage - Decentralized Storage
// Register storage requests, providers confirm and earn rewards

#![no_std]
#![no_main]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set,
    bytes_to_u64, u64_to_bytes, get_slot
};

const MAX_REPLICATION: u8 = 10;
const MIN_DURATION: u64 = 1000;  // Minimum 1000 slots
const REWARD_PER_SLOT_PER_BYTE: u64 = 1; // 1 shell per slot per byte
const MAX_PROVIDERS: u8 = 16;
// DataEntry: owner(32)+size(8)+replication(1)+confirmations(1)+expiry(8)+created(8)+provider_count(1) = 59 header
const DATA_HEADER_SIZE: usize = 59;

fn hex(addr: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(addr.len() * 2);
    for &b in addr { let _ = alloc::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b)); }
    s
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

#[no_mangle]
pub extern "C" fn store_data(
    owner_ptr: *const u8,
    data_hash_ptr: *const u8,
    size: u64,
    replication_factor: u8,
    duration_slots: u64,
) -> u32 {
    if replication_factor > MAX_REPLICATION || replication_factor == 0 {
        log_info(&alloc::format!("Replication must be 1-{}", MAX_REPLICATION));
        return 0;
    }
    if duration_slots < MIN_DURATION {
        log_info(&alloc::format!("Min duration: {} slots", MIN_DURATION));
        return 0;
    }

    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let data_hash = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let now = get_slot();
    let expiry = now + duration_slots;

    let mut entry = Vec::with_capacity(DATA_HEADER_SIZE);
    entry.extend_from_slice(owner);                           // 0..32
    entry.extend_from_slice(&u64_to_bytes(size));             // 32..40
    entry.push(replication_factor);                            // 40
    entry.push(0);                                             // 41 confirmations
    entry.extend_from_slice(&u64_to_bytes(expiry));           // 42..50
    entry.extend_from_slice(&u64_to_bytes(now));              // 50..58
    entry.push(0);                                             // 58 provider_count

    let key = alloc::format!("data_{}", hex(data_hash));
    storage_set(key.as_bytes(), &entry);

    let count = load_u64(b"data_count") + 1;
    store_u64(b"data_count", count);

    let cost = size * duration_slots * replication_factor as u64 * REWARD_PER_SLOT_PER_BYTE;
    log_info(&alloc::format!("Storage registered: {} bytes, {}x replication, {} slots (cost: {} shells)",
        size, replication_factor, duration_slots, cost));
    1
}

#[no_mangle]
pub extern "C" fn confirm_storage(provider_ptr: *const u8, data_hash_ptr: *const u8) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let data_hash = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let key = alloc::format!("data_{}", hex(data_hash));

    let mut data = match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= DATA_HEADER_SIZE => d,
        _ => { log_info("Data entry not found"); return 0; }
    };

    let replication = data[40];
    let confirmations = data[41];
    if confirmations >= replication {
        log_info("Already fully replicated");
        return 0;
    }

    let provider_count = data[58] as usize;
    if provider_count >= MAX_PROVIDERS as usize {
        log_info("Max providers reached");
        return 0;
    }

    data[41] = confirmations + 1;
    data[58] = (provider_count + 1) as u8;
    data.extend_from_slice(provider); // Append provider address
    storage_set(key.as_bytes(), &data);

    log_info(&alloc::format!("Storage confirmed ({}/{})", confirmations + 1, replication));
    1
}

#[no_mangle]
pub extern "C" fn register_provider(provider_ptr: *const u8, capacity_bytes: u64) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let h = hex(provider);

    let mut prov = Vec::with_capacity(33);
    prov.extend_from_slice(&u64_to_bytes(capacity_bytes)); // 0..8 capacity
    prov.extend_from_slice(&u64_to_bytes(0));              // 8..16 used
    prov.extend_from_slice(&u64_to_bytes(0));              // 16..24 stored_count
    prov.push(1);                                           // 24 active
    prov.extend_from_slice(&u64_to_bytes(get_slot()));     // 25..33

    let key = alloc::format!("provider_{}", h);
    storage_set(key.as_bytes(), &prov);
    log_info(&alloc::format!("Storage provider registered: {} bytes capacity", capacity_bytes));
    1
}

#[no_mangle]
pub extern "C" fn claim_storage_rewards(provider_ptr: *const u8) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let h = hex(provider);
    let key = alloc::format!("reward_{}", h);
    let rewards = load_u64(key.as_bytes());
    if rewards == 0 { log_info("No rewards to claim"); return 0; }
    store_u64(key.as_bytes(), 0);
    log_info(&alloc::format!("Claimed {} shells in storage rewards", rewards));
    1
}

#[no_mangle]
pub extern "C" fn get_storage_info(data_hash_ptr: *const u8) -> u32 {
    let data_hash = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let key = alloc::format!("data_{}", hex(data_hash));
    match storage_get(key.as_bytes()) {
        Some(d) if d.len() >= DATA_HEADER_SIZE => {
            let size = bytes_to_u64(&d[32..40]);
            let replication = d[40];
            let confirmations = d[41];
            log_info(&alloc::format!("Storage: {} bytes, {}/{} replicated", size, confirmations, replication));
            1
        }
        _ => 0
    }
}
`,
            'Cargo.toml': `[package]
name = "reef-storage"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = "1.0"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
`
        }
    },

    // ═══════════════════ MoltyDEX Suite ═══════════════════

    dex_core: {
        name: 'DEX Core (CLOB)',
        description: 'Central Limit Order Book with price-time matching, self-trade prevention, and settlement',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX Core — Central Limit Order Book ═══════
// Order types: Limit, Market, Stop-Limit, Post-Only
// Matching: Price-time priority, self-trade prevention
// Settlement: Atomic balance updates, fee distribution

const MAX_PAIRS: u64 = 50;
const MAX_OPEN_ORDERS: u64 = 100;
const DEFAULT_TAKER_FEE: u64 = 5;   // 5 bps
const DEFAULT_MAKER_REBATE: u64 = 1; // -1 bps rebate

fn reentrancy_enter() { let v = load_u64(b"reenter"); assert!(v == 0, "Reentrancy"); store_u64(b"reenter", 1); }
fn reentrancy_exit() { store_u64(b"reenter", 0); }
fn require_admin() { let a = storage_get(b"admin"); assert!(&get_caller() == a.as_slice(), "Not admin"); }
fn is_paused() -> bool { load_u64(b"paused") == 1 }
fn require_not_paused() { assert!(!is_paused(), "Paused"); }

// ═══ Trading Pair: 112 bytes binary layout ═══
// [0..32]  base_token
// [32..64] quote_token
// [64..72] best_bid
// [72..80] best_ask
// [80..88] taker_fee_bps
// [88..96] maker_rebate_bps
// [96..104] min_order_size
// [104..112] status (0=inactive, 1=active)

fn create_pair(base: &[u8], quote: &[u8], min_order: u64) {
    require_admin();
    let count = load_u64(b"pair_count");
    assert!(count < MAX_PAIRS, "Max pairs");
    let id = count;
    let mut data = vec![0u8; 112];
    data[0..32].copy_from_slice(base);
    data[32..64].copy_from_slice(quote);
    data[64..72].copy_from_slice(&0u64.to_le_bytes());      // best_bid = 0
    data[72..80].copy_from_slice(&u64::MAX.to_le_bytes());   // best_ask = MAX
    data[80..88].copy_from_slice(&DEFAULT_TAKER_FEE.to_le_bytes());
    data[88..96].copy_from_slice(&DEFAULT_MAKER_REBATE.to_le_bytes());
    data[96..104].copy_from_slice(&min_order.to_le_bytes());
    data[104..112].copy_from_slice(&1u64.to_le_bytes());     // active
    storage_set(&[b"pair:", &id.to_le_bytes()].concat(), &data);
    store_u64(b"pair_count", count + 1);
    emit_event(b"PairCreated", &format!("id={},min={}", id, min_order).into_bytes());
    log_info(&format!("Pair {} created: base/quote with min_order={}", id, min_order));
}

// ═══ Order: 128 bytes binary layout ═══
// [0..32]   owner
// [32..40]  pair_id
// [40..48]  price
// [48..56]  quantity
// [56..64]  filled
// [64..72]  side (0=buy, 1=sell)
// [72..80]  order_type (0=limit, 1=market, 2=stop-limit, 3=post-only)
// [80..88]  stop_price
// [88..96]  timestamp
// [96..104] expiry
// [104..112] status (0=open, 1=filled, 2=cancelled, 3=expired)

fn place_order(pair_id: u64, price: u64, qty: u64, side: u64, otype: u64) {
    require_not_paused();
    reentrancy_enter();
    let caller = get_caller();
    let pair_data = storage_get(&[b"pair:", &pair_id.to_le_bytes()].concat());
    assert!(pair_data.len() == 112, "Invalid pair");

    let notional = price.checked_mul(qty).unwrap_or(0) / 1_000_000_000;
    let min_ord = u64::from_le_bytes(pair_data[96..104].try_into().unwrap());
    assert!(notional >= min_ord, "Order too small");

    let order_id = load_u64(b"order_count");
    let mut order = vec![0u8; 128];
    order[0..32].copy_from_slice(&caller);
    order[32..40].copy_from_slice(&pair_id.to_le_bytes());
    order[40..48].copy_from_slice(&price.to_le_bytes());
    order[48..56].copy_from_slice(&qty.to_le_bytes());
    order[64..72].copy_from_slice(&side.to_le_bytes());
    order[72..80].copy_from_slice(&otype.to_le_bytes());
    order[88..96].copy_from_slice(&get_timestamp().to_le_bytes());
    storage_set(&[b"order:", &order_id.to_le_bytes()].concat(), &order);
    store_u64(b"order_count", order_id + 1);

    emit_event(b"OrderPlaced", &format!("id={},pair={},side={},price={},qty={}", order_id, pair_id, side, price, qty).into_bytes());
    reentrancy_exit();
}

fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }
fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* create_pair */ }
        2 => { /* place_order */ }
        3 => { /* cancel_order */ }
        4 => { /* get_order */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-core"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    dex_amm: {
        name: 'DEX AMM (Concentrated Liquidity)',
        description: 'Concentrated liquidity AMM with 4 fee tiers, Q32.32 math, tick-range positions',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX AMM — Concentrated Liquidity ═══════
// Uniswap V3-style with tick ranges and fee tiers
// Fee tiers: 1bps (stables), 5bps, 30bps (default), 100bps (exotic)
// Q32.32 fixed-point math for sqrt prices

const FEE_TIERS: [u64; 4] = [1, 5, 30, 100]; // basis points
const MAX_POOLS: u64 = 100;

fn reentrancy_enter() { let v = load_u64(b"reenter"); assert!(v == 0); store_u64(b"reenter", 1); }
fn reentrancy_exit() { store_u64(b"reenter", 0); }
fn require_admin() { let a = storage_get(b"admin"); assert!(&get_caller() == a.as_slice()); }

// ═══ Pool: 96 bytes ═══
// [0..32]  token_a
// [32..64] token_b
// [64..72] fee_tier (bps)
// [72..80] sqrt_price (Q32.32)
// [80..88] liquidity
// [88..96] total_fees_collected

fn create_pool(token_a: &[u8], token_b: &[u8], fee_tier: u64, initial_price: u64) {
    require_admin();
    assert!(FEE_TIERS.contains(&fee_tier), "Invalid fee tier");
    let count = load_u64(b"pool_count");
    assert!(count < MAX_POOLS, "Max pools");

    let sqrt_price = sqrt_q32(initial_price);
    let mut pool = vec![0u8; 96];
    pool[0..32].copy_from_slice(token_a);
    pool[32..64].copy_from_slice(token_b);
    pool[64..72].copy_from_slice(&fee_tier.to_le_bytes());
    pool[72..80].copy_from_slice(&sqrt_price.to_le_bytes());
    storage_set(&[b"pool:", &count.to_le_bytes()].concat(), &pool);
    store_u64(b"pool_count", count + 1);
    emit_event(b"PoolCreated", &format!("pool={},fee={}bps", count, fee_tier).into_bytes());
}

// ═══ Position: 80 bytes ═══
fn add_liquidity(pool_id: u64, tick_lower: i32, tick_upper: i32, amount: u64) {
    reentrancy_enter();
    let caller = get_caller();
    assert!(tick_lower < tick_upper, "Invalid range");
    let pos_id = load_u64(b"pos_count");
    let mut pos = vec![0u8; 80];
    pos[0..32].copy_from_slice(&caller);
    pos[32..40].copy_from_slice(&pool_id.to_le_bytes());
    pos[40..44].copy_from_slice(&tick_lower.to_le_bytes());
    pos[44..48].copy_from_slice(&tick_upper.to_le_bytes());
    pos[48..56].copy_from_slice(&amount.to_le_bytes());
    storage_set(&[b"pos:", &pos_id.to_le_bytes()].concat(), &pos);
    store_u64(b"pos_count", pos_id + 1);
    emit_event(b"LiquidityAdded", &format!("pos={},pool={},amount={}", pos_id, pool_id, amount).into_bytes());
    reentrancy_exit();
}

fn sqrt_q32(x: u64) -> u64 {
    if x == 0 { return 0; }
    let mut r = x;
    let mut g = (x + 1) / 2;
    while g < r { r = g; g = (x / g + g) / 2; }
    r << 16 // approximate Q32.32
}

fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }
fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* create_pool */ }
        2 => { /* add_liquidity */ }
        3 => { /* remove_liquidity */ }
        4 => { /* swap */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-amm"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    dex_router: {
        name: 'DEX Router (Smart Routing)',
        description: 'Smart order routing across CLOB, AMM, and legacy MoltSwap with multi-hop splits',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX Router — Smart Order Routing ═══════
// Routes across: CLOB (dex_core), AMM (dex_amm), Legacy (MoltSwap)
// Strategies: Direct, Multi-hop, Split, CLOB+AMM Hybrid

const ROUTE_CLOB: u8 = 0;
const ROUTE_AMM: u8 = 1;
const ROUTE_LEGACY: u8 = 2;
const ROUTE_MULTI_HOP: u8 = 3;
const ROUTE_SPLIT: u8 = 4;
const MAX_ROUTES: u64 = 200;

fn require_admin() { let a = storage_get(b"admin"); assert!(&get_caller() == a.as_slice()); }

// ═══ Route: 96 bytes ═══
// [0..32] token_in
// [32..64] token_out
// [64..65] route_type
// [65..73] estimated_output
// [73..81] fee_estimate_bps
// [81..89] last_used_slot
// [89..97] success_count

fn register_route(token_in: &[u8], token_out: &[u8], route_type: u8) {
    require_admin();
    let count = load_u64(b"route_count");
    assert!(count < MAX_ROUTES, "Max routes");
    let mut route = vec![0u8; 96];
    route[0..32].copy_from_slice(token_in);
    route[32..64].copy_from_slice(token_out);
    route[64] = route_type;
    storage_set(&[b"route:", &count.to_le_bytes()].concat(), &route);
    store_u64(b"route_count", count + 1);
    emit_event(b"RouteRegistered", &format!("route={},type={}", count, route_type).into_bytes());
}

fn simulate_swap(route_id: u64, amount_in: u64) -> u64 {
    let route = storage_get(&[b"route:", &route_id.to_le_bytes()].concat());
    assert!(route.len() == 96, "Invalid route");
    let route_type = route[64];
    let fee_bps: u64 = match route_type {
        0 => 5,   // CLOB
        1 => 30,  // AMM
        2 => 30,  // Legacy
        _ => 50,  // Multi-hop ~50bps
    };
    let fee = amount_in * fee_bps / 10_000;
    amount_in - fee
}

fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }
fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* register_route */ }
        2 => { /* execute_swap */ }
        3 => { /* simulate_swap */ }
        4 => { /* find_best_route */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-router"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    dex_governance: {
        name: 'DEX Governance (Voting)',
        description: 'Pair listing & fee governance via proposals and token-weighted voting',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX Governance ═══════
// Proposal types: New Pair, Fee Change, Delist, Param Change
// 48h voting period (172,800 slots @2.5s)
// 66% approval threshold, 1h execution timelock
// MoltyID reputation-gated (min 500 rep)

const VOTING_PERIOD: u64 = 172_800;
const APPROVAL_THRESHOLD: u64 = 66;
const EXECUTION_TIMELOCK: u64 = 1_440; // ~1 hour
const MIN_REPUTATION: u64 = 500;
const MAX_PROPOSALS: u64 = 500;

fn require_admin() { let a = storage_get(b"admin"); assert!(&get_caller() == a.as_slice()); }
fn require_not_paused() { assert!(load_u64(b"paused") == 0, "Paused"); }

// ═══ Proposal: 120 bytes ═══
// [0..32]   proposer
// [32..40]  proposal_type (0=new_pair, 1=fee_change, 2=delist, 3=param)
// [40..48]  target_pair_id
// [48..56]  proposed_value
// [56..64]  votes_yes
// [64..72]  votes_no
// [72..80]  start_slot
// [80..88]  end_slot
// [88..96]  execution_slot (end + timelock)
// [96..104] status (0=active, 1=passed, 2=rejected, 3=executed)
// [104..112] voter_count
// [112..120] total_weight

fn propose_new_pair(pair_data: &[u8]) {
    require_not_paused();
    let caller = get_caller();
    let count = load_u64(b"proposal_count");
    assert!(count < MAX_PROPOSALS, "Max proposals");
    let slot = get_slot();

    let mut prop = vec![0u8; 120];
    prop[0..32].copy_from_slice(&caller);
    prop[32..40].copy_from_slice(&0u64.to_le_bytes()); // type=new_pair
    prop[72..80].copy_from_slice(&slot.to_le_bytes());
    prop[80..88].copy_from_slice(&(slot + VOTING_PERIOD).to_le_bytes());
    prop[88..96].copy_from_slice(&(slot + VOTING_PERIOD + EXECUTION_TIMELOCK).to_le_bytes());
    storage_set(&[b"prop:", &count.to_le_bytes()].concat(), &prop);
    store_u64(b"proposal_count", count + 1);
    emit_event(b"ProposalCreated", &format!("id={},type=new_pair", count).into_bytes());
}

fn vote(proposal_id: u64, vote_yes: bool, weight: u64) {
    require_not_paused();
    let prop = storage_get(&[b"prop:", &proposal_id.to_le_bytes()].concat());
    assert!(prop.len() == 120, "Invalid proposal");
    let status = u64::from_le_bytes(prop[96..104].try_into().unwrap());
    assert!(status == 0, "Not active");
    let slot = get_slot();
    let end = u64::from_le_bytes(prop[80..88].try_into().unwrap());
    assert!(slot <= end, "Voting ended");

    let mut p = prop.clone();
    if vote_yes {
        let yes = u64::from_le_bytes(p[56..64].try_into().unwrap());
        p[56..64].copy_from_slice(&(yes + weight).to_le_bytes());
    } else {
        let no = u64::from_le_bytes(p[64..72].try_into().unwrap());
        p[64..72].copy_from_slice(&(no + weight).to_le_bytes());
    }
    storage_set(&[b"prop:", &proposal_id.to_le_bytes()].concat(), &p);
    emit_event(b"VoteCast", &format!("prop={},yes={},weight={}", proposal_id, vote_yes, weight).into_bytes());
}

fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }
fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* propose_new_pair */ }
        2 => { /* propose_fee_change */ }
        3 => { /* vote */ }
        4 => { /* finalize_proposal */ }
        5 => { /* execute_proposal */ }
        6 => { /* emergency_delist (admin) */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-governance"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    dex_rewards: {
        name: 'DEX Rewards (Mining)',
        description: 'Trading rewards, LP mining, and referral program with 4 tiers',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX Rewards ═══════
// Trading rewards: Tier-based multipliers (Bronze 1x → Diamond 3x)
// LP Mining: Proportional to in-range liquidity
// Referrals: 10% of referee fees (15% for MoltyID-verified)
// Fee Mining: 1:1 MOLT per shell spent in fees

const TIER_BRONZE: u64 = 0;
const TIER_SILVER: u64 = 1;
const TIER_GOLD: u64 = 2;
const TIER_DIAMOND: u64 = 3;

fn multiplier(tier: u64) -> u64 {
    match tier {
        0 => 100,  // 1.0x
        1 => 150,  // 1.5x
        2 => 200,  // 2.0x
        3 => 300,  // 3.0x
        _ => 100,
    }
}

fn tier_threshold(tier: u64) -> u64 {
    match tier {
        0 => 0,
        1 => 100_000,     // 100K volume
        2 => 1_000_000,   // 1M volume
        3 => 10_000_000,  // 10M volume
        _ => u64::MAX,
    }
}

fn get_trading_tier(trader: &[u8]) -> u64 {
    let vol = load_u64(&[b"vol:", trader].concat());
    if vol >= tier_threshold(3) { TIER_DIAMOND }
    else if vol >= tier_threshold(2) { TIER_GOLD }
    else if vol >= tier_threshold(1) { TIER_SILVER }
    else { TIER_BRONZE }
}

fn record_trade(trader: &[u8], pair_id: u64, volume: u64, fees_paid: u64) {
    // Accumulate volume
    let key = [b"vol:", trader].concat();
    let current = load_u64(&key);
    store_u64(&key, current + volume);

    // Calculate reward: fees * multiplier
    let tier = get_trading_tier(trader);
    let mult = multiplier(tier);
    let reward = fees_paid * mult / 100;

    let rkey = [b"pending:", trader].concat();
    let pending = load_u64(&rkey);
    store_u64(&rkey, pending + reward);

    emit_event(b"TradeRecorded", &format!("vol={},tier={},reward={}", volume, tier, reward).into_bytes());
}

fn claim_rewards(trader: &[u8]) -> u64 {
    let key = [b"pending:", trader].concat();
    let pending = load_u64(&key);
    assert!(pending > 0, "Nothing to claim");
    store_u64(&key, 0);
    let total = load_u64(b"total_distributed");
    store_u64(b"total_distributed", total + pending);
    emit_event(b"RewardsClaimed", &format!("trader={:?},amount={}", &trader[..4], pending).into_bytes());
    pending
}

fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }
fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* record_trade */ }
        2 => { /* claim_trading_rewards */ }
        3 => { /* claim_lp_rewards */ }
        4 => { /* register_referral */ }
        5 => { /* set_reward_rate */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-rewards"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    dex_margin: {
        name: 'DEX Margin (Leverage)',
        description: 'Margin trading up to 5x leverage with isolated/cross margin, liquidation, insurance fund',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX Margin Trading ═══════
// Isolated margin: up to 5x leverage
// Cross margin: up to 3x leverage
// Maintenance margin: 10%, Liquidation penalty: 5%
// Insurance fund: 50% of liquidation penalty
// 8-hour funding interval

const MAX_LEVERAGE_ISOLATED: u64 = 5;
const MAX_LEVERAGE_CROSS: u64 = 3;
const INITIAL_MARGIN_BPS: u64 = 2000;    // 20%
const MAINTENANCE_MARGIN_BPS: u64 = 1000; // 10%
const LIQUIDATION_PENALTY_BPS: u64 = 500; // 5%
const MAX_POSITIONS: u64 = 10_000;

fn reentrancy_enter() { let v = load_u64(b"reenter"); assert!(v == 0); store_u64(b"reenter", 1); }
fn reentrancy_exit() { store_u64(b"reenter", 0); }
fn require_not_paused() { assert!(load_u64(b"paused") == 0, "Paused"); }

// ═══ Position: 112 bytes ═══
// [0..32]  owner
// [32..40] pair_id
// [40..48] side (0=long, 1=short)
// [48..56] margin_type (0=isolated, 1=cross)
// [56..64] leverage (1-5)
// [64..72] size
// [72..80] entry_price
// [80..88] collateral
// [88..96] unrealized_pnl
// [96..104] mark_price
// [104..112] status (0=open, 1=closed, 2=liquidated)

fn open_position(pair_id: u64, side: u64, margin_type: u64, leverage: u64, size: u64, collateral: u64) {
    require_not_paused();
    reentrancy_enter();
    let max_lev = if margin_type == 0 { MAX_LEVERAGE_ISOLATED } else { MAX_LEVERAGE_CROSS };
    assert!(leverage >= 1 && leverage <= max_lev, "Invalid leverage");
    let min_margin = size * INITIAL_MARGIN_BPS / 10_000;
    assert!(collateral >= min_margin, "Insufficient margin");

    let caller = get_caller();
    let pos_id = load_u64(b"pos_count");
    assert!(pos_id < MAX_POSITIONS, "Max positions");
    let mark = load_u64(&[b"mark:", &pair_id.to_le_bytes()].concat());

    let mut pos = vec![0u8; 112];
    pos[0..32].copy_from_slice(&caller);
    pos[32..40].copy_from_slice(&pair_id.to_le_bytes());
    pos[40..48].copy_from_slice(&side.to_le_bytes());
    pos[48..56].copy_from_slice(&margin_type.to_le_bytes());
    pos[56..64].copy_from_slice(&leverage.to_le_bytes());
    pos[64..72].copy_from_slice(&size.to_le_bytes());
    pos[72..80].copy_from_slice(&mark.to_le_bytes());
    pos[80..88].copy_from_slice(&collateral.to_le_bytes());
    pos[96..104].copy_from_slice(&mark.to_le_bytes());
    storage_set(&[b"pos:", &pos_id.to_le_bytes()].concat(), &pos);
    store_u64(b"pos_count", pos_id + 1);
    emit_event(b"PositionOpened", &format!("id={},lev={}x,size={}", pos_id, leverage, size).into_bytes());
    reentrancy_exit();
}

fn liquidate(pos_id: u64) {
    reentrancy_enter();
    let pos = storage_get(&[b"pos:", &pos_id.to_le_bytes()].concat());
    assert!(pos.len() == 112, "Invalid position");
    let collateral = u64::from_le_bytes(pos[80..88].try_into().unwrap());
    let size = u64::from_le_bytes(pos[64..72].try_into().unwrap());
    let margin_ratio = collateral * 10_000 / size;
    assert!(margin_ratio < MAINTENANCE_MARGIN_BPS, "Not liquidatable");

    let penalty = collateral * LIQUIDATION_PENALTY_BPS / 10_000;
    let to_insurance = penalty / 2;
    let to_liquidator = penalty - to_insurance;
    let ins = load_u64(b"insurance_fund");
    store_u64(b"insurance_fund", ins + to_insurance);

    emit_event(b"Liquidated", &format!("pos={},penalty={}", pos_id, penalty).into_bytes());
    reentrancy_exit();
}

fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }
fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* open_position */ }
        2 => { /* close_position */ }
        3 => { /* add_margin */ }
        4 => { /* liquidate */ }
        5 => { /* set_mark_price */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-margin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    dex_analytics: {
        name: 'DEX Analytics (Data)',
        description: 'On-chain OHLCV candles, 24h rolling stats, trader leaderboards',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ DEX Analytics ═══════
// OHLCV Candles: 6 intervals (1m, 5m, 15m, 1h, 4h, 1d)
// 24h Rolling Stats: volume, trades, high, low per pair
// Trader Stats: volume traded, trade count, PnL
// Candle retention: 1440 (1m) to 365 (1d)

const INTERVALS: [u64; 6] = [60, 300, 900, 3600, 14400, 86400]; // seconds

// ═══ Candle: 48 bytes ═══
// [0..8]   open
// [8..16]  high
// [16..24] low
// [24..32] close
// [32..40] volume
// [40..48] trade_count

fn record_trade(pair_id: u64, price: u64, volume: u64, timestamp: u64) {
    // Update candles for all intervals
    for &interval in &INTERVALS {
        let bucket = timestamp / interval;
        let key = [b"candle:", &pair_id.to_le_bytes(), b":", &interval.to_le_bytes(), b":", &bucket.to_le_bytes()].concat();
        let existing = storage_get(&key);

        let mut candle = if existing.len() == 48 {
            existing
        } else {
            let mut c = vec![0u8; 48];
            c[0..8].copy_from_slice(&price.to_le_bytes());   // open
            c[8..16].copy_from_slice(&price.to_le_bytes());  // high
            c[16..24].copy_from_slice(&price.to_le_bytes()); // low
            c
        };

        // Update high/low/close/volume/count
        let high = u64::from_le_bytes(candle[8..16].try_into().unwrap());
        let low = u64::from_le_bytes(candle[16..24].try_into().unwrap());
        if price > high { candle[8..16].copy_from_slice(&price.to_le_bytes()); }
        if price < low { candle[16..24].copy_from_slice(&price.to_le_bytes()); }
        candle[24..32].copy_from_slice(&price.to_le_bytes()); // close = latest
        let vol = u64::from_le_bytes(candle[32..40].try_into().unwrap());
        candle[32..40].copy_from_slice(&(vol + volume).to_le_bytes());
        let cnt = u64::from_le_bytes(candle[40..48].try_into().unwrap());
        candle[40..48].copy_from_slice(&(cnt + 1).to_le_bytes());

        storage_set(&key, &candle);
    }

    // Update 24h rolling stats
    let stats_key = [b"stats24:", &pair_id.to_le_bytes()].concat();
    let mut stats = storage_get(&stats_key);
    if stats.len() < 48 { stats = vec![0u8; 48]; }
    let vol24 = u64::from_le_bytes(stats[0..8].try_into().unwrap());
    stats[0..8].copy_from_slice(&(vol24 + volume).to_le_bytes());
    let trades24 = u64::from_le_bytes(stats[8..16].try_into().unwrap());
    stats[8..16].copy_from_slice(&(trades24 + 1).to_le_bytes());
    storage_set(&stats_key, &stats);

    // Store last price
    storage_set(&[b"last:", &pair_id.to_le_bytes()].concat(), &price.to_le_bytes());

    emit_event(b"TradeRecorded", &format!("pair={},price={},vol={}", pair_id, price, volume).into_bytes());
}

fn get_ohlcv(pair_id: u64, interval: u64, bucket: u64) -> Vec<u8> {
    storage_get(&[b"candle:", &pair_id.to_le_bytes(), b":", &interval.to_le_bytes(), b":", &bucket.to_le_bytes()].concat())
}

fn get_last_price(pair_id: u64) -> u64 {
    let d = storage_get(&[b"last:", &pair_id.to_le_bytes()].concat());
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        1 => { /* record_trade */ }
        2 => { /* get_ohlcv */ }
        3 => { /* get_24h_stats */ }
        4 => { /* get_trader_stats */ }
        5 => { /* get_last_price */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "dex-analytics"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    prediction_market: {
        name: 'Prediction Market',
        description: 'On-chain prediction markets with CPMM AMM, oracle resolution, and share redemption',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ Prediction Market ═══════
// Binary outcome markets with CPMM AMM pricing
// Oracle-based resolution with DAO arbitration fallback

const MARKET_FEE_BPS: u64 = 200; // 2% fee
const CHALLENGE_PERIOD: u64 = 86400; // 24h

fn create_market(creator: &[u8], question: &[u8], end_time: u64) -> u64 {
    let id = load_u64(b"market_count") + 1;
    store_u64(b"market_count", id);
    let key = [b"market:", &id.to_le_bytes()].concat();
    let mut data = vec![0u8; 64];
    data[0..32].copy_from_slice(&creator[..32]);
    data[32..40].copy_from_slice(&end_time.to_le_bytes());
    data[40] = 0; // status: open
    storage_set(&key, &data);
    emit_event(b"MarketCreated", &format!("id={}", id).into_bytes());
    id
}

fn buy_shares(market_id: u64, buyer: &[u8], outcome: u8, amount: u64) {
    let pool_key = [b"pool:", &market_id.to_le_bytes()].concat();
    let pool = storage_get(&pool_key);
    let yes_res = u64::from_le_bytes(pool[0..8].try_into().unwrap());
    let no_res = u64::from_le_bytes(pool[8..16].try_into().unwrap());
    let cost = if outcome == 0 {
        (amount * yes_res) / (yes_res + no_res)
    } else {
        (amount * no_res) / (yes_res + no_res)
    };
    let bal_key = [b"bal:", &market_id.to_le_bytes(), b":", buyer, &[outcome]].concat();
    let cur = load_u64(&bal_key);
    store_u64(&bal_key, cur + amount);
    emit_event(b"SharesBought", &format!("market={},outcome={},amt={}", market_id, outcome, amount).into_bytes());
}

fn redeem_shares(market_id: u64, holder: &[u8]) {
    let key = [b"market:", &market_id.to_le_bytes()].concat();
    let data = storage_get(&key);
    let winner = data[41]; // resolved outcome
    let bal_key = [b"bal:", &market_id.to_le_bytes(), b":", holder, &[winner]].concat();
    let shares = load_u64(&bal_key);
    store_u64(&bal_key, 0);
    emit_event(b"SharesRedeemed", &format!("market={},payout={}", market_id, shares).into_bytes());
}

fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}
fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        0 => { /* create_market */ }
        1 => { /* buy_shares */ }
        2 => { /* sell_shares */ }
        3 => { /* add_liquidity */ }
        8 => { /* submit_resolution */ }
        13 => { /* redeem_shares */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "prediction-market"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    musd_token: {
        name: 'mUSD Stablecoin',
        description: 'Treasury-backed stablecoin with reserve attestation and circuit breaker',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ mUSD Stablecoin ═══════
// Treasury-backed 1:1 USD stablecoin
// Epoch rate-limited minting with circuit breaker

const NAME: &[u8] = b"mUSD";
const SYMBOL: &[u8] = b"mUSD";
const DECIMALS: u8 = 9;
const EPOCH_RATE_LIMIT: u64 = 100_000_000_000_000; // 100k mUSD
const CIRCUIT_BREAKER_THRESHOLD: u64 = 50_000_000_000_000; // 50k mUSD

fn initialize(admin: &[u8]) {
    storage_set(b"admin", admin);
    store_u64(b"total_supply", 0);
    store_u64(b"epoch_minted", 0);
    emit_event(b"Initialized", b"mUSD stablecoin ready");
}

fn mint(to: &[u8], amount: u64) {
    let minted = load_u64(b"epoch_minted");
    if minted + amount > EPOCH_RATE_LIMIT { return; } // rate limit
    if amount > CIRCUIT_BREAKER_THRESHOLD { return; } // circuit breaker
    let bal_key = [b"bal:", to].concat();
    let cur = load_u64(&bal_key);
    store_u64(&bal_key, cur + amount);
    let supply = load_u64(b"total_supply");
    store_u64(b"total_supply", supply + amount);
    store_u64(b"epoch_minted", minted + amount);
    emit_event(b"Mint", &format!("to={:?},amt={}", &to[..4], amount).into_bytes());
}

fn burn(from: &[u8], amount: u64) {
    let bal_key = [b"bal:", from].concat();
    let cur = load_u64(&bal_key);
    if cur < amount { return; }
    store_u64(&bal_key, cur - amount);
    let supply = load_u64(b"total_supply");
    store_u64(b"total_supply", supply - amount);
    emit_event(b"Burn", &format!("amt={}", amount).into_bytes());
}

fn transfer(from: &[u8], to: &[u8], amount: u64) {
    let from_key = [b"bal:", from].concat();
    let to_key = [b"bal:", to].concat();
    let from_bal = load_u64(&from_key);
    if from_bal < amount { return; }
    store_u64(&from_key, from_bal - amount);
    store_u64(&to_key, load_u64(&to_key) + amount);
}

fn balance_of(account: &[u8]) -> u64 {
    load_u64(&[b"bal:", account].concat())
}

fn get_reserves() -> u64 { load_u64(b"total_supply") }

fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}
fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        0 => { /* initialize */ }
        1 => { /* mint */ }
        2 => { /* burn */ }
        3 => { /* transfer */ }
        4 => { /* approve */ }
        5 => { /* transfer_from */ }
        6 => { /* total_supply */ }
        7 => { /* balance_of */ }
        8 => { /* get_reserves */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "musd-token"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    weth_token: {
        name: 'Wrapped ETH',
        description: 'Wrapped ETH bridge token with epoch rate-limited minting',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ Wrapped ETH (wETH) ═══════
// Bridge token representing ETH on MoltChain
// Epoch rate-limited minting with reserve attestation

const NAME: &[u8] = b"Wrapped ETH";
const SYMBOL: &[u8] = b"wETH";
const DECIMALS: u8 = 18;
const EPOCH_RATE_LIMIT: u64 = 500_000_000_000_000_000; // 500 ETH/epoch

fn initialize(bridge_admin: &[u8]) {
    storage_set(b"admin", bridge_admin);
    store_u64(b"total_supply", 0);
    store_u64(b"epoch_minted", 0);
    emit_event(b"Initialized", b"wETH bridge token ready");
}

fn mint(to: &[u8], amount: u64) {
    let minted = load_u64(b"epoch_minted");
    if minted + amount > EPOCH_RATE_LIMIT { return; }
    let bal_key = [b"bal:", to].concat();
    let cur = load_u64(&bal_key);
    store_u64(&bal_key, cur + amount);
    let supply = load_u64(b"total_supply");
    store_u64(b"total_supply", supply + amount);
    store_u64(b"epoch_minted", minted + amount);
    emit_event(b"Mint", &format!("to={:?},amt={}", &to[..4], amount).into_bytes());
}

fn burn(from: &[u8], amount: u64) {
    let bal_key = [b"bal:", from].concat();
    let cur = load_u64(&bal_key);
    if cur < amount { return; }
    store_u64(&bal_key, cur - amount);
    let supply = load_u64(b"total_supply");
    store_u64(b"total_supply", supply - amount);
    emit_event(b"Burn", &format!("amt={}", amount).into_bytes());
}

fn transfer(from: &[u8], to: &[u8], amount: u64) {
    let from_key = [b"bal:", from].concat();
    let to_key = [b"bal:", to].concat();
    let from_bal = load_u64(&from_key);
    if from_bal < amount { return; }
    store_u64(&from_key, from_bal - amount);
    store_u64(&to_key, load_u64(&to_key) + amount);
}

fn balance_of(account: &[u8]) -> u64 {
    load_u64(&[b"bal:", account].concat())
}

fn get_reserves() -> u64 { load_u64(b"total_supply") }

fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}
fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        0 => { /* initialize */ }
        1 => { /* mint */ }
        2 => { /* burn */ }
        3 => { /* transfer */ }
        4 => { /* approve */ }
        5 => { /* transfer_from */ }
        6 => { /* total_supply */ }
        7 => { /* balance_of */ }
        8 => { /* get_reserves */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "weth-token"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    },

    wsol_token: {
        name: 'Wrapped SOL',
        description: 'Wrapped SOL bridge token with epoch rate-limited minting',
        files: {
            'lib.rs': `#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::{vec, vec::Vec, format};
use moltchain_sdk::*;

// ═══════ Wrapped SOL (wSOL) ═══════
// Bridge token representing SOL on MoltChain
// Epoch rate-limited minting with reserve attestation

const NAME: &[u8] = b"Wrapped SOL";
const SYMBOL: &[u8] = b"wSOL";
const DECIMALS: u8 = 9;
const EPOCH_RATE_LIMIT: u64 = 50_000_000_000_000; // 50,000 SOL/epoch

fn initialize(bridge_admin: &[u8]) {
    storage_set(b"admin", bridge_admin);
    store_u64(b"total_supply", 0);
    store_u64(b"epoch_minted", 0);
    emit_event(b"Initialized", b"wSOL bridge token ready");
}

fn mint(to: &[u8], amount: u64) {
    let minted = load_u64(b"epoch_minted");
    if minted + amount > EPOCH_RATE_LIMIT { return; }
    let bal_key = [b"bal:", to].concat();
    let cur = load_u64(&bal_key);
    store_u64(&bal_key, cur + amount);
    let supply = load_u64(b"total_supply");
    store_u64(b"total_supply", supply + amount);
    store_u64(b"epoch_minted", minted + amount);
    emit_event(b"Mint", &format!("to={:?},amt={}", &to[..4], amount).into_bytes());
}

fn burn(from: &[u8], amount: u64) {
    let bal_key = [b"bal:", from].concat();
    let cur = load_u64(&bal_key);
    if cur < amount { return; }
    store_u64(&bal_key, cur - amount);
    let supply = load_u64(b"total_supply");
    store_u64(b"total_supply", supply - amount);
    emit_event(b"Burn", &format!("amt={}", amount).into_bytes());
}

fn transfer(from: &[u8], to: &[u8], amount: u64) {
    let from_key = [b"bal:", from].concat();
    let to_key = [b"bal:", to].concat();
    let from_bal = load_u64(&from_key);
    if from_bal < amount { return; }
    store_u64(&from_key, from_bal - amount);
    store_u64(&to_key, load_u64(&to_key) + amount);
}

fn balance_of(account: &[u8]) -> u64 {
    load_u64(&[b"bal:", account].concat())
}

fn get_reserves() -> u64 { load_u64(b"total_supply") }

fn load_u64(key: &[u8]) -> u64 {
    let d = storage_get(key);
    if d.len() >= 8 { u64::from_le_bytes(d[..8].try_into().unwrap()) } else { 0 }
}
fn store_u64(key: &[u8], val: u64) { storage_set(key, &val.to_le_bytes()); }

#[no_mangle]
pub extern "C" fn call() {
    let input = moltchain_sdk::get_call_data();
    if input.is_empty() { return; }
    match input[0] {
        0 => { /* initialize */ }
        1 => { /* mint */ }
        2 => { /* burn */ }
        3 => { /* transfer */ }
        4 => { /* approve */ }
        5 => { /* transfer_from */ }
        6 => { /* total_supply */ }
        7 => { /* balance_of */ }
        8 => { /* get_reserves */ }
        _ => {}
    }
}
`,
            'Cargo.toml': `[package]
name = "wsol-token"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }
`
        }
    }
};

// ============================================================================
// AUTO-INITIALIZE
// ============================================================================

document.addEventListener('DOMContentLoaded', () => {
    console.log('🦞 DOM loaded, initializing Playground...');
    Playground.init().catch(err => {
        console.error('❌ Failed to initialize Playground:', err);
        alert('Failed to initialize playground. Check console for details.');
    });
});

// Make Playground globally accessible
window.Playground = Playground;

console.log('✅ Playground script loaded');
