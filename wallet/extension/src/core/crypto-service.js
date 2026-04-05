import {
  derivePublicKey as derivePqPublicKey,
  isValidAddress as isValidPqAddress,
  publicKeyToAddress as pqPublicKeyToAddress,
  signMessage as signPqMessage,
} from '../../shared/pq.mjs';

// BIP39 wordlist (full 2048 words) — must match website's crypto.js exactly
const BIP39_WORDLIST = [
  'abandon', 'ability', 'able', 'about', 'above', 'absent', 'absorb', 'abstract', 'absurd', 'abuse',
  'access', 'accident', 'account', 'accuse', 'achieve', 'acid', 'acoustic', 'acquire', 'across', 'act',
  'action', 'actor', 'actress', 'actual', 'adapt', 'add', 'addict', 'address', 'adjust', 'admit',
  'adult', 'advance', 'advice', 'aerobic', 'affair', 'afford', 'afraid', 'again', 'age', 'agent',
  'agree', 'ahead', 'aim', 'air', 'airport', 'aisle', 'alarm', 'album', 'alcohol', 'alert',
  'alien', 'all', 'alley', 'allow', 'almost', 'alone', 'alpha', 'already', 'also', 'alter',
  'always', 'amateur', 'amazing', 'among', 'amount', 'amused', 'analyst', 'anchor', 'ancient', 'anger',
  'angle', 'angry', 'animal', 'ankle', 'announce', 'annual', 'another', 'answer', 'antenna', 'antique',
  'anxiety', 'any', 'apart', 'apology', 'appear', 'apple', 'approve', 'april', 'arch', 'arctic',
  'area', 'arena', 'argue', 'arm', 'armed', 'armor', 'army', 'around', 'arrange', 'arrest',
  'arrive', 'arrow', 'art', 'artefact', 'artist', 'artwork', 'ask', 'aspect', 'assault', 'asset',
  'assist', 'assume', 'asthma', 'athlete', 'atom', 'attack', 'attend', 'attitude', 'attract', 'auction',
  'audit', 'august', 'aunt', 'author', 'auto', 'autumn', 'average', 'avocado', 'avoid', 'awake',
  'aware', 'away', 'awesome', 'awful', 'awkward', 'axis', 'baby', 'bachelor', 'bacon', 'badge',
  'bag', 'balance', 'balcony', 'ball', 'bamboo', 'banana', 'banner', 'bar', 'barely', 'bargain',
  'barrel', 'base', 'basic', 'basket', 'battle', 'beach', 'bean', 'beauty', 'because', 'become',
  'beef', 'before', 'begin', 'behave', 'behind', 'believe', 'below', 'belt', 'bench', 'benefit',
  'best', 'betray', 'better', 'between', 'beyond', 'bicycle', 'bid', 'bike', 'bind', 'biology',
  'bird', 'birth', 'bitter', 'black', 'blade', 'blame', 'blanket', 'blast', 'bleak', 'bless',
  'blind', 'blood', 'blossom', 'blouse', 'blue', 'blur', 'blush', 'board', 'boat', 'body',
  'boil', 'bomb', 'bone', 'bonus', 'book', 'boost', 'border', 'boring', 'borrow', 'boss',
  'bottom', 'bounce', 'box', 'boy', 'bracket', 'brain', 'brand', 'brass', 'brave', 'bread',
  'breeze', 'brick', 'bridge', 'brief', 'bright', 'bring', 'brisk', 'broccoli', 'broken', 'bronze',
  'broom', 'brother', 'brown', 'brush', 'bubble', 'buddy', 'budget', 'buffalo', 'build', 'bulb',
  'bulk', 'bullet', 'bundle', 'bunker', 'burden', 'burger', 'burst', 'bus', 'business', 'busy',
  'butter', 'buyer', 'buzz', 'cabbage', 'cabin', 'cable', 'cactus', 'cage', 'cake', 'call',
  'calm', 'camera', 'camp', 'can', 'canal', 'cancel', 'candy', 'cannon', 'canoe', 'canvas',
  'canyon', 'capable', 'capital', 'captain', 'car', 'carbon', 'card', 'cargo', 'carpet', 'carry',
  'cart', 'case', 'cash', 'casino', 'castle', 'casual', 'cat', 'catalog', 'catch', 'category',
  'cattle', 'caught', 'cause', 'caution', 'cave', 'ceiling', 'celery', 'cement', 'census', 'century',
  'cereal', 'certain', 'chair', 'chalk', 'champion', 'change', 'chaos', 'chapter', 'charge', 'chase',
  'chat', 'cheap', 'check', 'cheese', 'chef', 'cherry', 'chest', 'chicken', 'chief', 'child',
  'chimney', 'choice', 'choose', 'chronic', 'chuckle', 'chunk', 'churn', 'cigar', 'cinnamon', 'circle',
  'citizen', 'city', 'civil', 'claim', 'clap', 'clarify', 'claw', 'clay', 'clean', 'clerk',
  'clever', 'click', 'client', 'cliff', 'climb', 'clinic', 'clip', 'clock', 'clog', 'close',
  'cloth', 'cloud', 'clown', 'club', 'clump', 'cluster', 'clutch', 'coach', 'coast', 'coconut',
  'code', 'coffee', 'coil', 'coin', 'collect', 'color', 'column', 'combine', 'come', 'comfort',
  'comic', 'common', 'company', 'concert', 'conduct', 'confirm', 'congress', 'connect', 'consider', 'control',
  'convince', 'cook', 'cool', 'copper', 'copy', 'coral', 'core', 'corn', 'correct', 'cost',
  'cotton', 'couch', 'country', 'couple', 'course', 'cousin', 'cover', 'coyote', 'crack', 'cradle',
  'craft', 'cram', 'crane', 'crash', 'crater', 'crawl', 'crazy', 'cream', 'credit', 'creek',
  'crew', 'cricket', 'crime', 'crisp', 'critic', 'crop', 'cross', 'crouch', 'crowd', 'crucial',
  'cruel', 'cruise', 'crumble', 'crunch', 'crush', 'cry', 'crystal', 'cube', 'culture', 'cup',
  'cupboard', 'curious', 'current', 'curtain', 'curve', 'cushion', 'custom', 'cute', 'cycle', 'dad',
  'damage', 'damp', 'dance', 'danger', 'daring', 'dash', 'daughter', 'dawn', 'day', 'deal',
  'debate', 'debris', 'decade', 'december', 'decide', 'decline', 'decorate', 'decrease', 'deer', 'defense',
  'define', 'defy', 'degree', 'delay', 'deliver', 'demand', 'demise', 'denial', 'dentist', 'deny',
  'depart', 'depend', 'deposit', 'depth', 'deputy', 'derive', 'describe', 'desert', 'design', 'desk',
  'despair', 'destroy', 'detail', 'detect', 'develop', 'device', 'devote', 'diagram', 'dial', 'diamond',
  'diary', 'dice', 'diesel', 'diet', 'differ', 'digital', 'dignity', 'dilemma', 'dinner', 'dinosaur',
  'direct', 'dirt', 'disagree', 'discover', 'disease', 'dish', 'dismiss', 'disorder', 'display', 'distance',
  'divert', 'divide', 'divorce', 'dizzy', 'doctor', 'document', 'dog', 'doll', 'dolphin', 'domain',
  'donate', 'donkey', 'donor', 'door', 'dose', 'double', 'dove', 'draft', 'dragon', 'drama',
  'drastic', 'draw', 'dream', 'dress', 'drift', 'drill', 'drink', 'drip', 'drive', 'drop',
  'drum', 'dry', 'duck', 'dumb', 'dune', 'during', 'dust', 'dutch', 'duty', 'dwarf',
  'dynamic', 'eager', 'eagle', 'early', 'earn', 'earth', 'easily', 'east', 'easy', 'echo',
  'ecology', 'economy', 'edge', 'edit', 'educate', 'effort', 'egg', 'eight', 'either', 'elbow',
  'elder', 'electric', 'elegant', 'element', 'elephant', 'elevator', 'elite', 'else', 'embark', 'embody',
  'embrace', 'emerge', 'emotion', 'employ', 'empower', 'empty', 'enable', 'enact', 'end', 'endless',
  'endorse', 'enemy', 'energy', 'enforce', 'engage', 'engine', 'enhance', 'enjoy', 'enlist', 'enough',
  'enrich', 'enroll', 'ensure', 'enter', 'entire', 'entry', 'envelope', 'episode', 'equal', 'equip',
  'era', 'erase', 'erode', 'erosion', 'error', 'erupt', 'escape', 'essay', 'essence', 'estate',
  'eternal', 'ethics', 'evidence', 'evil', 'evoke', 'evolve', 'exact', 'example', 'excess', 'exchange',
  'excite', 'exclude', 'excuse', 'execute', 'exercise', 'exhaust', 'exhibit', 'exile', 'exist', 'exit',
  'exotic', 'expand', 'expect', 'expire', 'explain', 'expose', 'express', 'extend', 'extra', 'eye',
  'eyebrow', 'fabric', 'face', 'faculty', 'fade', 'faint', 'faith', 'fall', 'false', 'fame',
  'family', 'famous', 'fan', 'fancy', 'fantasy', 'farm', 'fashion', 'fat', 'fatal', 'father',
  'fatigue', 'fault', 'favorite', 'feature', 'february', 'federal', 'fee', 'feed', 'feel', 'female',
  'fence', 'festival', 'fetch', 'fever', 'few', 'fiber', 'fiction', 'field', 'figure', 'file',
  'film', 'filter', 'final', 'find', 'fine', 'finger', 'finish', 'fire', 'firm', 'first',
  'fiscal', 'fish', 'fit', 'fitness', 'fix', 'flag', 'flame', 'flash', 'flat', 'flavor',
  'flee', 'flight', 'flip', 'float', 'flock', 'floor', 'flower', 'fluid', 'flush', 'fly',
  'foam', 'focus', 'fog', 'foil', 'fold', 'follow', 'food', 'foot', 'force', 'forest',
  'forget', 'fork', 'fortune', 'forum', 'forward', 'fossil', 'foster', 'found', 'fox', 'fragile',
  'frame', 'frequent', 'fresh', 'friend', 'fringe', 'frog', 'front', 'frost', 'frown', 'frozen',
  'fruit', 'fuel', 'fun', 'funny', 'furnace', 'fury', 'future', 'gadget', 'gain', 'galaxy',
  'gallery', 'game', 'gap', 'garage', 'garbage', 'garden', 'garlic', 'garment', 'gas', 'gasp',
  'gate', 'gather', 'gauge', 'gaze', 'general', 'genius', 'genre', 'gentle', 'genuine', 'gesture',
  'ghost', 'giant', 'gift', 'giggle', 'ginger', 'giraffe', 'girl', 'give', 'glad', 'glance',
  'glare', 'glass', 'glide', 'glimpse', 'globe', 'gloom', 'glory', 'glove', 'glow', 'glue',
  'goat', 'goddess', 'gold', 'good', 'goose', 'gorilla', 'gospel', 'gossip', 'govern', 'gown',
  'grab', 'grace', 'grain', 'grant', 'grape', 'grass', 'gravity', 'great', 'green', 'grid',
  'grief', 'grit', 'grocery', 'group', 'grow', 'grunt', 'guard', 'guess', 'guide', 'guilt',
  'guitar', 'gun', 'gym', 'habit', 'hair', 'half', 'hammer', 'hamster', 'hand', 'happy',
  'harbor', 'hard', 'harsh', 'harvest', 'hat', 'have', 'hawk', 'hazard', 'head', 'health',
  'heart', 'heavy', 'hedgehog', 'height', 'hello', 'helmet', 'help', 'hen', 'hero', 'hidden',
  'high', 'hill', 'hint', 'hip', 'hire', 'history', 'hobby', 'hockey', 'hold', 'hole',
  'holiday', 'hollow', 'home', 'honey', 'hood', 'hope', 'horn', 'horror', 'horse', 'hospital',
  'host', 'hotel', 'hour', 'hover', 'hub', 'huge', 'human', 'humble', 'humor', 'hundred',
  'hungry', 'hunt', 'hurdle', 'hurry', 'hurt', 'husband', 'hybrid', 'ice', 'icon', 'idea',
  'identify', 'idle', 'ignore', 'ill', 'illegal', 'illness', 'image', 'imitate', 'immense', 'immune',
  'impact', 'impose', 'improve', 'impulse', 'inch', 'include', 'income', 'increase', 'index', 'indicate',
  'indoor', 'industry', 'infant', 'inflict', 'inform', 'inhale', 'inherit', 'initial', 'inject', 'injury',
  'inmate', 'inner', 'innocent', 'input', 'inquiry', 'insane', 'insect', 'inside', 'inspire', 'install',
  'intact', 'interest', 'into', 'invest', 'invite', 'involve', 'iron', 'island', 'isolate', 'issue',
  'item', 'ivory', 'jacket', 'jaguar', 'jar', 'jazz', 'jealous', 'jeans', 'jelly', 'jewel',
  'job', 'join', 'joke', 'journey', 'joy', 'judge', 'juice', 'jump', 'jungle', 'junior',
  'junk', 'just', 'kangaroo', 'keen', 'keep', 'ketchup', 'key', 'kick', 'kid', 'kidney',
  'kind', 'kingdom', 'kiss', 'kit', 'kitchen', 'kite', 'kitten', 'kiwi', 'knee', 'knife',
  'knock', 'know', 'lab', 'label', 'labor', 'ladder', 'lady', 'lake', 'lamp', 'language',
  'laptop', 'large', 'later', 'latin', 'laugh', 'laundry', 'lava', 'law', 'lawn', 'lawsuit',
  'layer', 'lazy', 'leader', 'leaf', 'learn', 'leave', 'lecture', 'left', 'leg', 'legal',
  'legend', 'leisure', 'lemon', 'lend', 'length', 'lens', 'leopard', 'lesson', 'letter', 'level',
  'liar', 'liberty', 'library', 'license', 'life', 'lift', 'light', 'like', 'limb', 'limit',
  'link', 'lion', 'liquid', 'list', 'little', 'live', 'lizard', 'load', 'loan', 'lobster',
  'local', 'lock', 'logic', 'lonely', 'long', 'loop', 'lottery', 'loud', 'lounge', 'love',
  'loyal', 'lucky', 'luggage', 'lumber', 'lunar', 'lunch', 'luxury', 'lyrics', 'machine', 'mad',
  'magic', 'magnet', 'maid', 'mail', 'main', 'major', 'make', 'mammal', 'man', 'manage',
  'mandate', 'mango', 'mansion', 'manual', 'maple', 'marble', 'march', 'margin', 'marine', 'market',
  'marriage', 'mask', 'mass', 'master', 'match', 'material', 'math', 'matrix', 'matter', 'maximum',
  'maze', 'meadow', 'mean', 'measure', 'meat', 'mechanic', 'medal', 'media', 'melody', 'melt',
  'member', 'memory', 'mention', 'menu', 'mercy', 'merge', 'merit', 'merry', 'mesh', 'message',
  'metal', 'method', 'middle', 'midnight', 'milk', 'million', 'mimic', 'mind', 'minimum', 'minor',
  'minute', 'miracle', 'mirror', 'misery', 'miss', 'mistake', 'mix', 'mixed', 'mixture', 'mobile',
  'model', 'modify', 'mom', 'moment', 'monitor', 'monkey', 'monster', 'month', 'moon', 'moral',
  'more', 'morning', 'mosquito', 'mother', 'motion', 'motor', 'mountain', 'mouse', 'move', 'movie',
  'much', 'muffin', 'mule', 'multiply', 'muscle', 'museum', 'mushroom', 'music', 'must', 'mutual',
  'myself', 'mystery', 'myth', 'naive', 'name', 'napkin', 'narrow', 'nasty', 'nation', 'nature',
  'near', 'neck', 'need', 'negative', 'neglect', 'neither', 'nephew', 'nerve', 'nest', 'net',
  'network', 'neutral', 'never', 'news', 'next', 'nice', 'night', 'noble', 'noise', 'nominee',
  'noodle', 'normal', 'north', 'nose', 'notable', 'note', 'nothing', 'notice', 'novel', 'now',
  'nuclear', 'number', 'nurse', 'nut', 'oak', 'obey', 'object', 'oblige', 'obscure', 'observe',
  'obtain', 'obvious', 'occur', 'ocean', 'october', 'odor', 'off', 'offer', 'office', 'often',
  'oil', 'okay', 'old', 'olive', 'olympic', 'omit', 'once', 'one', 'onion', 'online',
  'only', 'open', 'opera', 'opinion', 'oppose', 'option', 'orange', 'orbit', 'orchard', 'order',
  'ordinary', 'organ', 'orient', 'original', 'orphan', 'ostrich', 'other', 'outdoor', 'outer', 'output',
  'outside', 'oval', 'oven', 'over', 'own', 'owner', 'oxygen', 'oyster', 'ozone', 'pact',
  'paddle', 'page', 'pair', 'palace', 'palm', 'panda', 'panel', 'panic', 'panther', 'paper',
  'parade', 'parent', 'park', 'parrot', 'party', 'pass', 'patch', 'path', 'patient', 'patrol',
  'pattern', 'pause', 'pave', 'payment', 'peace', 'peanut', 'pear', 'peasant', 'pelican', 'pen',
  'penalty', 'pencil', 'people', 'pepper', 'perfect', 'permit', 'person', 'pet', 'phone', 'photo',
  'phrase', 'physical', 'piano', 'picnic', 'picture', 'piece', 'pig', 'pigeon', 'pill', 'pilot',
  'pink', 'pioneer', 'pipe', 'pistol', 'pitch', 'pizza', 'place', 'planet', 'plastic', 'plate',
  'play', 'please', 'pledge', 'pluck', 'plug', 'plunge', 'poem', 'poet', 'point', 'polar',
  'pole', 'police', 'pond', 'pony', 'pool', 'popular', 'portion', 'position', 'possible', 'post',
  'potato', 'pottery', 'poverty', 'powder', 'power', 'practice', 'praise', 'predict', 'prefer', 'prepare',
  'present', 'pretty', 'prevent', 'price', 'pride', 'primary', 'print', 'priority', 'prison', 'private',
  'prize', 'problem', 'process', 'produce', 'profit', 'program', 'project', 'promote', 'proof', 'property',
  'prosper', 'protect', 'proud', 'provide', 'public', 'pudding', 'pull', 'pulp', 'pulse', 'pumpkin',
  'punch', 'pupil', 'puppy', 'purchase', 'purity', 'purpose', 'purse', 'push', 'put', 'puzzle',
  'pyramid', 'quality', 'quantum', 'quarter', 'question', 'quick', 'quit', 'quiz', 'quote', 'rabbit',
  'raccoon', 'race', 'rack', 'radar', 'radio', 'rail', 'rain', 'raise', 'rally', 'ramp',
  'ranch', 'random', 'range', 'rapid', 'rare', 'rate', 'rather', 'raven', 'raw', 'razor',
  'ready', 'real', 'reason', 'rebel', 'rebuild', 'recall', 'receive', 'recipe', 'record', 'recycle',
  'reduce', 'reflect', 'reform', 'refuse', 'region', 'regret', 'regular', 'reject', 'relax', 'release',
  'relief', 'rely', 'remain', 'remember', 'remind', 'remove', 'render', 'renew', 'rent', 'reopen',
  'repair', 'repeat', 'replace', 'report', 'require', 'rescue', 'resemble', 'resist', 'resource', 'response',
  'result', 'retire', 'retreat', 'return', 'reunion', 'reveal', 'review', 'reward', 'rhythm', 'rib',
  'ribbon', 'rice', 'rich', 'ride', 'ridge', 'rifle', 'right', 'rigid', 'ring', 'riot',
  'ripple', 'risk', 'ritual', 'rival', 'river', 'road', 'roast', 'robot', 'robust', 'rocket',
  'romance', 'roof', 'rookie', 'room', 'rose', 'rotate', 'rough', 'round', 'route', 'royal',
  'rubber', 'rude', 'rug', 'rule', 'run', 'runway', 'rural', 'sad', 'saddle', 'sadness',
  'safe', 'sail', 'salad', 'salmon', 'salon', 'salt', 'salute', 'same', 'sample', 'sand',
  'satisfy', 'satoshi', 'sauce', 'sausage', 'save', 'say', 'scale', 'scan', 'scare', 'scatter',
  'scene', 'scheme', 'school', 'science', 'scissors', 'scorpion', 'scout', 'scrap', 'screen', 'script',
  'scrub', 'sea', 'search', 'season', 'seat', 'second', 'secret', 'section', 'security', 'seed',
  'seek', 'segment', 'select', 'sell', 'seminar', 'senior', 'sense', 'sentence', 'series', 'service',
  'session', 'settle', 'setup', 'seven', 'shadow', 'shaft', 'shallow', 'share', 'shed', 'shell',
  'sheriff', 'shield', 'shift', 'shine', 'ship', 'shiver', 'shock', 'shoe', 'shoot', 'shop',
  'short', 'shoulder', 'shove', 'shrimp', 'shrug', 'shuffle', 'shy', 'sibling', 'sick', 'side',
  'siege', 'sight', 'sign', 'silent', 'silk', 'silly', 'silver', 'similar', 'simple', 'since',
  'sing', 'siren', 'sister', 'situate', 'six', 'size', 'skate', 'sketch', 'ski', 'skill',
  'skin', 'skirt', 'skull', 'slab', 'slam', 'sleep', 'slender', 'slice', 'slide', 'slight',
  'slim', 'slogan', 'slot', 'slow', 'slush', 'small', 'smart', 'smile', 'smoke', 'smooth',
  'snack', 'snake', 'snap', 'sniff', 'snow', 'soap', 'soccer', 'social', 'sock', 'soda',
  'soft', 'solar', 'soldier', 'solid', 'solution', 'solve', 'someone', 'song', 'soon', 'sorry',
  'sort', 'soul', 'sound', 'soup', 'source', 'south', 'space', 'spare', 'spatial', 'spawn',
  'speak', 'special', 'speed', 'spell', 'spend', 'sphere', 'spice', 'spider', 'spike', 'spin',
  'spirit', 'split', 'spoil', 'sponsor', 'spoon', 'sport', 'spot', 'spray', 'spread', 'spring',
  'spy', 'square', 'squeeze', 'squirrel', 'stable', 'stadium', 'staff', 'stage', 'stairs', 'stamp',
  'stand', 'start', 'state', 'stay', 'steak', 'steel', 'stem', 'step', 'stereo', 'stick',
  'still', 'sting', 'stock', 'stomach', 'stone', 'stool', 'story', 'stove', 'strategy', 'street',
  'strike', 'strong', 'struggle', 'student', 'stuff', 'stumble', 'style', 'subject', 'submit', 'subway',
  'success', 'such', 'sudden', 'suffer', 'sugar', 'suggest', 'suit', 'summer', 'sun', 'sunny',
  'sunset', 'super', 'supply', 'supreme', 'sure', 'surface', 'surge', 'surprise', 'surround', 'survey',
  'suspect', 'sustain', 'swallow', 'swamp', 'swap', 'swarm', 'swear', 'sweet', 'swift', 'swim',
  'swing', 'switch', 'sword', 'symbol', 'symptom', 'syrup', 'system', 'table', 'tackle', 'tag',
  'tail', 'talent', 'talk', 'tank', 'tape', 'target', 'task', 'taste', 'tattoo', 'taxi',
  'teach', 'team', 'tell', 'ten', 'tenant', 'tennis', 'tent', 'term', 'test', 'text',
  'thank', 'that', 'theme', 'then', 'theory', 'there', 'they', 'thing', 'this', 'thought',
  'three', 'thrive', 'throw', 'thumb', 'thunder', 'ticket', 'tide', 'tiger', 'tilt', 'timber',
  'time', 'tiny', 'tip', 'tired', 'tissue', 'title', 'toast', 'tobacco', 'today', 'toddler',
  'toe', 'together', 'toilet', 'token', 'tomato', 'tomorrow', 'tone', 'tongue', 'tonight', 'tool',
  'tooth', 'top', 'topic', 'topple', 'torch', 'tornado', 'tortoise', 'toss', 'total', 'tourist',
  'toward', 'tower', 'town', 'toy', 'track', 'trade', 'traffic', 'tragic', 'train', 'transfer',
  'trap', 'trash', 'travel', 'tray', 'treat', 'tree', 'trend', 'trial', 'tribe', 'trick',
  'trigger', 'trim', 'trip', 'trophy', 'trouble', 'truck', 'true', 'truly', 'trumpet', 'trust',
  'truth', 'try', 'tube', 'tuition', 'tumble', 'tuna', 'tunnel', 'turkey', 'turn', 'turtle',
  'twelve', 'twenty', 'twice', 'twin', 'twist', 'two', 'type', 'typical', 'ugly', 'umbrella',
  'unable', 'unaware', 'uncle', 'uncover', 'under', 'undo', 'unfair', 'unfold', 'unhappy', 'uniform',
  'unique', 'unit', 'universe', 'unknown', 'unlock', 'until', 'unusual', 'unveil', 'update', 'upgrade',
  'uphold', 'upon', 'upper', 'upset', 'urban', 'urge', 'usage', 'use', 'used', 'useful',
  'useless', 'usual', 'utility', 'vacant', 'vacuum', 'vague', 'valid', 'valley', 'valve', 'van',
  'vanish', 'vapor', 'various', 'vast', 'vault', 'vehicle', 'velvet', 'vendor', 'venture', 'venue',
  'verb', 'verify', 'version', 'very', 'vessel', 'veteran', 'viable', 'vibrant', 'vicious', 'victory',
  'video', 'view', 'village', 'vintage', 'violin', 'virtual', 'virus', 'visa', 'visit', 'visual',
  'vital', 'vivid', 'vocal', 'voice', 'void', 'volcano', 'volume', 'vote', 'voyage', 'wage',
  'wagon', 'wait', 'walk', 'wall', 'walnut', 'want', 'warfare', 'warm', 'warrior', 'wash',
  'wasp', 'waste', 'water', 'wave', 'way', 'wealth', 'weapon', 'wear', 'weasel', 'weather',
  'web', 'wedding', 'weekend', 'weird', 'welcome', 'west', 'wet', 'whale', 'what', 'wheat',
  'wheel', 'when', 'where', 'whip', 'whisper', 'wide', 'width', 'wife', 'wild', 'will',
  'win', 'window', 'wine', 'wing', 'wink', 'winner', 'winter', 'wire', 'wisdom', 'wise',
  'wish', 'witness', 'wolf', 'woman', 'wonder', 'wood', 'wool', 'word', 'work', 'world',
  'worry', 'worth', 'wrap', 'wreck', 'wrestle', 'wrist', 'write', 'wrong', 'yard', 'year',
  'yellow', 'you', 'young', 'youth', 'zebra', 'zero', 'zone', 'zoo'
];

const BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

export function bytesToHex(bytes) {
  return Array.from(bytes).map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

export function hexToBytes(hex) {
  const cleaned = hex.startsWith('0x') ? hex.slice(2) : hex;
  if (cleaned.length % 2 !== 0) {
    throw new Error('Invalid hex format');
  }
  const bytes = new Uint8Array(cleaned.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(cleaned.substr(i * 2, 2), 16);
  }
  return bytes;
}

async function sha256(inputBytes) {
  const digest = await crypto.subtle.digest('SHA-256', inputBytes);
  return new Uint8Array(digest);
}

function sha256Sync(bytes) {
  const K = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ];
  const H = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ];
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  const toUint8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const bitLen = toUint8.length * 8;
  const withOne = toUint8.length + 1;
  const padLen = (64 - ((withOne + 8) % 64)) % 64;
  const padded = new Uint8Array(withOne + padLen + 8);
  padded.set(toUint8, 0);
  padded[toUint8.length] = 0x80;

  const hi = Math.floor(bitLen / 0x100000000);
  const lo = bitLen >>> 0;
  const lenOffset = padded.length - 8;
  padded[lenOffset] = (hi >>> 24) & 0xff;
  padded[lenOffset + 1] = (hi >>> 16) & 0xff;
  padded[lenOffset + 2] = (hi >>> 8) & 0xff;
  padded[lenOffset + 3] = hi & 0xff;
  padded[lenOffset + 4] = (lo >>> 24) & 0xff;
  padded[lenOffset + 5] = (lo >>> 16) & 0xff;
  padded[lenOffset + 6] = (lo >>> 8) & 0xff;
  padded[lenOffset + 7] = lo & 0xff;

  const W = new Uint32Array(64);
  for (let i = 0; i < padded.length; i += 64) {
    for (let t = 0; t < 16; t++) {
      const j = i + t * 4;
      W[t] = ((padded[j] << 24) | (padded[j + 1] << 16) | (padded[j + 2] << 8) | padded[j + 3]) >>> 0;
    }
    for (let t = 16; t < 64; t++) {
      const s0 = rotr(W[t - 15], 7) ^ rotr(W[t - 15], 18) ^ (W[t - 15] >>> 3);
      const s1 = rotr(W[t - 2], 17) ^ rotr(W[t - 2], 19) ^ (W[t - 2] >>> 10);
      W[t] = (((W[t - 16] + s0) >>> 0) + ((W[t - 7] + s1) >>> 0)) >>> 0;
    }

    let [a, b, c, d, e, f, g, h] = H;
    for (let t = 0; t < 64; t++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const temp1 = (((((h + S1) >>> 0) + ch) >>> 0) + ((K[t] + W[t]) >>> 0)) >>> 0;
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (S0 + maj) >>> 0;

      h = g;
      g = f;
      f = e;
      e = (d + temp1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (temp1 + temp2) >>> 0;
    }

    H[0] = (H[0] + a) >>> 0;
    H[1] = (H[1] + b) >>> 0;
    H[2] = (H[2] + c) >>> 0;
    H[3] = (H[3] + d) >>> 0;
    H[4] = (H[4] + e) >>> 0;
    H[5] = (H[5] + f) >>> 0;
    H[6] = (H[6] + g) >>> 0;
    H[7] = (H[7] + h) >>> 0;
  }

  const out = new Uint8Array(32);
  H.forEach((value, index) => {
    out[index * 4] = (value >>> 24) & 0xff;
    out[index * 4 + 1] = (value >>> 16) & 0xff;
    out[index * 4 + 2] = (value >>> 8) & 0xff;
    out[index * 4 + 3] = value & 0xff;
  });
  return out;
}

export function base58Encode(buffer) {
  if (!buffer || buffer.length === 0) return '';

  const digits = [0];
  for (let i = 0; i < buffer.length; i++) {
    let carry = buffer[i];
    for (let j = 0; j < digits.length; j++) {
      carry += digits[j] << 8;
      digits[j] = carry % 58;
      carry = (carry / 58) | 0;
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = (carry / 58) | 0;
    }
  }

  let output = '';
  for (let i = 0; buffer[i] === 0 && i < buffer.length - 1; i++) {
    output += BASE58_ALPHABET[0];
  }
  for (let i = digits.length - 1; i >= 0; i--) {
    output += BASE58_ALPHABET[digits[i]];
  }
  return output;
}

export function base58Decode(string) {
  if (!string || string.length === 0) return new Uint8Array(0);

  const bytes = [0];
  for (let i = 0; i < string.length; i++) {
    const value = BASE58_ALPHABET.indexOf(string[i]);
    if (value === -1) {
      throw new Error(`Invalid base58 character: ${string[i]}`);
    }

    let carry = value;
    for (let j = 0; j < bytes.length; j++) {
      carry += bytes[j] * 58;
      bytes[j] = carry & 0xff;
      carry >>= 8;
    }
    while (carry > 0) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }

  for (let i = 0; string[i] === BASE58_ALPHABET[0] && i < string.length - 1; i++) {
    bytes.push(0);
  }

  return new Uint8Array(bytes.reverse());
}

export async function generateMnemonic() {
  // AUDIT-FIX FE-3: BIP39-compliant mnemonic with proper SHA-256 checksum
  const entropy = new Uint8Array(16); // 128 bits
  crypto.getRandomValues(entropy);

  // SHA-256 checksum
  const hashBuffer = await crypto.subtle.digest('SHA-256', entropy);
  const hashBytes = new Uint8Array(hashBuffer);
  const checksumBits = 4; // 128 / 32 = 4 checksum bits

  // Concatenate entropy + checksum bits into a bit string (132 bits = 12 × 11-bit words)
  let bits = '';
  for (let i = 0; i < entropy.length; i++) {
    bits += entropy[i].toString(2).padStart(8, '0');
  }
  bits += hashBytes[0].toString(2).padStart(8, '0').slice(0, checksumBits);

  // Extract 12 × 11-bit indices
  const words = [];
  for (let i = 0; i < 12; i++) {
    const idx = parseInt(bits.slice(i * 11, (i + 1) * 11), 2);
    words.push(BIP39_WORDLIST[idx]);
  }

  return words.join(' ');
}

export function isValidMnemonic(mnemonic) {
  const words = mnemonic.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length !== 12 || !words.every((word) => BIP39_WORDLIST.includes(word))) {
    return false;
  }

  let bits = '';
  for (const word of words) {
    const idx = BIP39_WORDLIST.indexOf(word);
    bits += idx.toString(2).padStart(11, '0');
  }

  const entropyBits = bits.slice(0, 128);
  const checksumBits = bits.slice(128, 132);
  const entropy = new Uint8Array(16);
  for (let index = 0; index < 16; index++) {
    entropy[index] = parseInt(entropyBits.slice(index * 8, (index + 1) * 8), 2);
  }

  const expectedChecksum = sha256Sync(entropy)[0].toString(2).padStart(8, '0').slice(0, 4);
  return checksumBits === expectedChecksum;
}

/**
 * AUDIT-FIX I2-01: Derive an ML-DSA-65 keypair from mnemonic using BIP39-compliant PBKDF2.
 * Replaces SHA-512 single-hash with PBKDF2-HMAC-SHA512 (2048 iterations) per BIP39 spec.
 * @param {string} mnemonic - BIP39 mnemonic phrase
 * @param {string} passphrase - Optional BIP39 passphrase (default: "")
 */
export async function mnemonicToKeypair(mnemonic, passphrase = '') {
  const encoder = new TextEncoder();
  // BIP39 spec: NFKD-normalize the mnemonic, salt = "mnemonic" + passphrase
  const mnemonicBytes = encoder.encode(mnemonic.normalize('NFKD').trim());
  const saltBytes = encoder.encode('mnemonic' + passphrase);

  // Import mnemonic as PBKDF2 key material
  const keyMaterial = await crypto.subtle.importKey(
    'raw', mnemonicBytes, 'PBKDF2', false, ['deriveBits']
  );

  // BIP39: PBKDF2-HMAC-SHA512, 2048 iterations, 512-bit (64-byte) output
  const seedBuffer = await crypto.subtle.deriveBits(
    { name: 'PBKDF2', salt: saltBytes, iterations: 2048, hash: 'SHA-512' },
    keyMaterial,
    512  // 64 bytes × 8 = 512 bits
  );

  // Take first 32 bytes as the native PQ seed
  const seedBytes = new Uint8Array(seedBuffer).slice(0, 32);
  const privateKeyHex = bytesToHex(seedBytes);

  const publicKeyBytes = derivePqPublicKey(seedBytes);
  const publicKeyHex = bytesToHex(publicKeyBytes);
  const address = await pqPublicKeyToAddress(publicKeyBytes);

  return {
    privateKey: privateKeyHex,
    publicKey: publicKeyHex,
    address
  };
}

export async function privateKeyToKeypair(privateKeyHex) {
  const seedBytes = hexToBytes(String(privateKeyHex || '').trim());
  if (seedBytes.length !== 32) {
    throw new Error('Private key must be 32 bytes (64 hex chars)');
  }

  const publicKeyBytes = derivePqPublicKey(seedBytes);
  return {
    privateKey: bytesToHex(seedBytes),
    publicKey: bytesToHex(publicKeyBytes),
    address: await pqPublicKeyToAddress(publicKeyBytes)
  };
}

async function deriveKey(password, salt) {
  const passwordKey = await crypto.subtle.importKey(
    'raw',
    new TextEncoder().encode(password),
    'PBKDF2',
    false,
    ['deriveKey']
  );

  return crypto.subtle.deriveKey(
    {
      name: 'PBKDF2',
      salt,
      iterations: 100000,
      hash: 'SHA-256'
    },
    passwordKey,
    {
      name: 'AES-GCM',
      length: 256
    },
    false,
    ['encrypt', 'decrypt']
  );
}

export async function encryptPrivateKey(privateKeyHex, password) {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveKey(password, salt);

  const encrypted = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv },
    key,
    new TextEncoder().encode(privateKeyHex)
  );

  return {
    encrypted: bytesToHex(new Uint8Array(encrypted)),
    salt: bytesToHex(salt),
    iv: bytesToHex(iv)
  };
}

export async function decryptPrivateKey(encryptedData, password) {
  const key = await deriveKey(password, hexToBytes(encryptedData.salt));

  try {
    const decrypted = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: hexToBytes(encryptedData.iv) },
      key,
      hexToBytes(encryptedData.encrypted)
    );

    return new TextDecoder().decode(decrypted);
  } catch {
    throw new Error('Invalid password');
  }
}

export function generateId() {
  return crypto.randomUUID();
}

/**
 * Keccak-256 hash (pure JS — matches js-sha3 output).
 * Returns 64-char hex string.
 */
export function keccak256(input) {
  const RC = [
    0x00000001n, 0x00008082n, 0x0000808An, 0x80008000n,
    0x0000808Bn, 0x80000001n, 0x80008081n, 0x00008009n,
    0x0000008An, 0x00000088n, 0x80008009n, 0x8000000An,
    0x8000808Bn, 0x0000008Bn, 0x00008089n, 0x00008003n,
    0x00008002n, 0x00000080n, 0x0000800An, 0x8000000An,
    0x80008081n, 0x00008080n, 0x80000001n, 0x80008008n
  ];
  const ROT = [
    [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]
  ];
  const state = new BigUint64Array(25);
  const rate = 136; // bytes (1088 bits for keccak-256)

  // Padding
  const data = input instanceof Uint8Array ? input : new TextEncoder().encode(input);
  const padded = new Uint8Array(Math.ceil((data.length + 1) / rate) * rate);
  padded.set(data);
  padded[data.length] = 0x01;
  padded[padded.length - 1] |= 0x80;

  // Absorb
  for (let offset = 0; offset < padded.length; offset += rate) {
    for (let i = 0; i < rate / 8; i++) {
      let v = 0n;
      for (let b = 0; b < 8; b++) v |= BigInt(padded[offset + i * 8 + b]) << BigInt(b * 8);
      state[i] ^= v;
    }
    // Keccak-f[1600]
    for (let round = 0; round < 24; round++) {
      // theta
      const C = new BigUint64Array(5);
      for (let x = 0; x < 5; x++) C[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
      for (let x = 0; x < 5; x++) {
        const D = C[(x + 4) % 5] ^ ((C[(x + 1) % 5] << 1n) | (C[(x + 1) % 5] >> 63n));
        for (let y = 0; y < 25; y += 5) state[y + x] ^= D;
      }
      // rho + pi
      const T = new BigUint64Array(25);
      for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) {
        const r = BigInt(ROT[x][y]);
        const v = state[y * 5 + x];
        T[x * 5 + ((2 * x + 3 * y) % 5)] = r ? ((v << r) | (v >> (64n - r))) : v;
      }
      // chi
      for (let y = 0; y < 25; y += 5)
        for (let x = 0; x < 5; x++)
          state[y + x] = T[y + x] ^ ((~T[y + (x + 1) % 5]) & T[y + (x + 2) % 5]);
      // iota
      state[0] ^= RC[round];
    }
  }

  // Squeeze — 32 bytes
  const hash = new Uint8Array(32);
  for (let i = 0; i < 4; i++) {
    const v = state[i];
    for (let b = 0; b < 8; b++) hash[i * 8 + b] = Number((v >> BigInt(b * 8)) & 0xFFn);
  }
  return Array.from(hash).map(b => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Derive EVM address from a Lichen base58 address.
 * keccak256(32-byte pubkey) → last 20 bytes → 0x-prefixed hex.
 */
export function generateEVMAddress(base58Address) {
  try {
    const pubkeyBytes = base58Decode(base58Address);
    if (pubkeyBytes.length !== 32) return null;
    const hashHex = keccak256(pubkeyBytes);
    return '0x' + hashHex.slice(-40);
  } catch {
    return null;
  }
}

export function isValidAddress(address) {
  return isValidPqAddress(address);
}

export async function signTransaction(privateKeyHex, messageBytes) {
  const payload = messageBytes instanceof Uint8Array ? messageBytes : new Uint8Array(messageBytes);
  return signPqMessage(privateKeyHex, payload);
}
