// BIP39 wordlist (full 2048 words) — must match website's crypto.js exactly
const BIP39_WORDLIST = [
    'abandon','ability','able','about','above','absent','absorb','abstract','absurd','abuse',
    'access','accident','account','accuse','achieve','acid','acoustic','acquire','across','act',
    'action','actor','actress','actual','adapt','add','addict','address','adjust','admit',
    'adult','advance','advice','aerobic','affair','afford','afraid','again','age','agent',
    'agree','ahead','aim','air','airport','aisle','alarm','album','alcohol','alert',
    'alien','all','alley','allow','almost','alone','alpha','already','also','alter',
    'always','amateur','amazing','among','amount','amused','analyst','anchor','ancient','anger',
    'angle','angry','animal','ankle','announce','annual','another','answer','antenna','antique',
    'anxiety','any','apart','apology','appear','apple','approve','april','arch','arctic',
    'area','arena','argue','arm','armed','armor','army','around','arrange','arrest',
    'arrive','arrow','art','artefact','artist','artwork','ask','aspect','assault','asset',
    'assist','assume','asthma','athlete','atom','attack','attend','attitude','attract','auction',
    'audit','august','aunt','author','auto','autumn','average','avocado','avoid','awake',
    'aware','away','awesome','awful','awkward','axis','baby','bachelor','bacon','badge',
    'bag','balance','balcony','ball','bamboo','banana','banner','bar','barely','bargain',
    'barrel','base','basic','basket','battle','beach','bean','beauty','because','become',
    'beef','before','begin','behave','behind','believe','below','belt','bench','benefit',
    'best','betray','better','between','beyond','bicycle','bid','bike','bind','biology',
    'bird','birth','bitter','black','blade','blame','blanket','blast','bleak','bless',
    'blind','blood','blossom','blouse','blue','blur','blush','board','boat','body',
    'boil','bomb','bone','bonus','book','boost','border','boring','borrow','boss',
    'bottom','bounce','box','boy','bracket','brain','brand','brass','brave','bread',
    'breeze','brick','bridge','brief','bright','bring','brisk','broccoli','broken','bronze',
    'broom','brother','brown','brush','bubble','buddy','budget','buffalo','build','bulb',
    'bulk','bullet','bundle','bunker','burden','burger','burst','bus','business','busy',
    'butter','buyer','buzz','cabbage','cabin','cable','cactus','cage','cake','call',
    'calm','camera','camp','can','canal','cancel','candy','cannon','canoe','canvas',
    'canyon','capable','capital','captain','car','carbon','card','cargo','carpet','carry',
    'cart','case','cash','casino','castle','casual','cat','catalog','catch','category',
    'cattle','caught','cause','caution','cave','ceiling','celery','cement','census','century',
    'cereal','certain','chair','chalk','champion','change','chaos','chapter','charge','chase',
    'chat','cheap','check','cheese','chef','cherry','chest','chicken','chief','child',
    'chimney','choice','choose','chronic','chuckle','chunk','churn','cigar','cinnamon','circle',
    'citizen','city','civil','claim','clap','clarify','claw','clay','clean','clerk',
    'clever','click','client','cliff','climb','clinic','clip','clock','clog','close',
    'cloth','cloud','clown','club','clump','cluster','clutch','coach','coast','coconut',
    'code','coffee','coil','coin','collect','color','column','combine','come','comfort',
    'comic','common','company','concert','conduct','confirm','congress','connect','consider','control',
    'convince','cook','cool','copper','copy','coral','core','corn','correct','cost',
    'cotton','couch','country','couple','course','cousin','cover','coyote','crack','cradle',
    'craft','cram','crane','crash','crater','crawl','crazy','cream','credit','creek',
    'crew','cricket','crime','crisp','critic','crop','cross','crouch','crowd','crucial',
    'cruel','cruise','crumble','crunch','crush','cry','crystal','cube','culture','cup',
    'cupboard','curious','current','curtain','curve','cushion','custom','cute','cycle','dad',
    'damage','damp','dance','danger','daring','dash','daughter','dawn','day','deal',
    'debate','debris','decade','december','decide','decline','decorate','decrease','deer','defense',
    'define','defy','degree','delay','deliver','demand','demise','denial','dentist','deny',
    'depart','depend','deposit','depth','deputy','derive','describe','desert','design','desk',
    'despair','destroy','detail','detect','develop','device','devote','diagram','dial','diamond',
    'diary','dice','diesel','diet','differ','digital','dignity','dilemma','dinner','dinosaur',
    'direct','dirt','disagree','discover','disease','dish','dismiss','disorder','display','distance',
    'divert','divide','divorce','dizzy','doctor','document','dog','doll','dolphin','domain',
    'donate','donkey','donor','door','dose','double','dove','draft','dragon','drama',
    'drastic','draw','dream','dress','drift','drill','drink','drip','drive','drop',
    'drum','dry','duck','dumb','dune','during','dust','dutch','duty','dwarf',
    'dynamic','eager','eagle','early','earn','earth','easily','east','easy','echo',
    'ecology','economy','edge','edit','educate','effort','egg','eight','either','elbow',
    'elder','electric','elegant','element','elephant','elevator','elite','else','embark','embody',
    'embrace','emerge','emotion','employ','empower','empty','enable','enact','end','endless',
    'endorse','enemy','energy','enforce','engage','engine','enhance','enjoy','enlist','enough',
    'enrich','enroll','ensure','enter','entire','entry','envelope','episode','equal','equip',
    'era','erase','erode','erosion','error','erupt','escape','essay','essence','estate',
    'eternal','ethics','evidence','evil','evoke','evolve','exact','example','excess','exchange',
    'excite','exclude','excuse','execute','exercise','exhaust','exhibit','exile','exist','exit',
    'exotic','expand','expect','expire','explain','expose','express','extend','extra','eye',
    'eyebrow','fabric','face','faculty','fade','faint','faith','fall','false','fame',
    'family','famous','fan','fancy','fantasy','farm','fashion','fat','fatal','father',
    'fatigue','fault','favorite','feature','february','federal','fee','feed','feel','female',
    'fence','festival','fetch','fever','few','fiber','fiction','field','figure','file',
    'film','filter','final','find','fine','finger','finish','fire','firm','first',
    'fiscal','fish','fit','fitness','fix','flag','flame','flash','flat','flavor',
    'flee','flight','flip','float','flock','floor','flower','fluid','flush','fly',
    'foam','focus','fog','foil','fold','follow','food','foot','force','forest',
    'forget','fork','fortune','forum','forward','fossil','foster','found','fox','fragile',
    'frame','frequent','fresh','friend','fringe','frog','front','frost','frown','frozen',
    'fruit','fuel','fun','funny','furnace','fury','future','gadget','gain','galaxy',
    'gallery','game','gap','garage','garbage','garden','garlic','garment','gas','gasp',
    'gate','gather','gauge','gaze','general','genius','genre','gentle','genuine','gesture',
    'ghost','giant','gift','giggle','ginger','giraffe','girl','give','glad','glance',
    'glare','glass','glide','glimpse','globe','gloom','glory','glove','glow','glue',
    'goat','goddess','gold','good','goose','gorilla','gospel','gossip','govern','gown',
    'grab','grace','grain','grant','grape','grass','gravity','great','green','grid',
    'grief','grit','grocery','group','grow','grunt','guard','guess','guide','guilt',
    'guitar','gun','gym','habit','hair','half','hammer','hamster','hand','happy',
    'harbor','hard','harsh','harvest','hat','have','hawk','hazard','head','health',
    'heart','heavy','hedgehog','height','hello','helmet','help','hen','hero','hidden',
    'high','hill','hint','hip','hire','history','hobby','hockey','hold','hole',
    'holiday','hollow','home','honey','hood','hope','horn','horror','horse','hospital',
    'host','hotel','hour','hover','hub','huge','human','humble','humor','hundred',
    'hungry','hunt','hurdle','hurry','hurt','husband','hybrid','ice','icon','idea',
    'identify','idle','ignore','ill','illegal','illness','image','imitate','immense','immune',
    'impact','impose','improve','impulse','inch','include','income','increase','index','indicate',
    'indoor','industry','infant','inflict','inform','inhale','inherit','initial','inject','injury',
    'inmate','inner','innocent','input','inquiry','insane','insect','inside','inspire','install',
    'intact','interest','into','invest','invite','involve','iron','island','isolate','issue',
    'item','ivory','jacket','jaguar','jar','jazz','jealous','jeans','jelly','jewel',
    'job','join','joke','journey','joy','judge','juice','jump','jungle','junior',
    'junk','just','kangaroo','keen','keep','ketchup','key','kick','kid','kidney',
    'kind','kingdom','kiss','kit','kitchen','kite','kitten','kiwi','knee','knife',
    'knock','know','lab','label','labor','ladder','lady','lake','lamp','language',
    'laptop','large','later','latin','laugh','laundry','lava','law','lawn','lawsuit',
    'layer','lazy','leader','leaf','learn','leave','lecture','left','leg','legal',
    'legend','leisure','lemon','lend','length','lens','leopard','lesson','letter','level',
    'liar','liberty','library','license','life','lift','light','like','limb','limit',
    'link','lion','liquid','list','little','live','lizard','load','loan','lobster',
    'local','lock','logic','lonely','long','loop','lottery','loud','lounge','love',
    'loyal','lucky','luggage','lumber','lunar','lunch','luxury','lyrics','machine','mad',
    'magic','magnet','maid','mail','main','major','make','mammal','man','manage',
    'mandate','mango','mansion','manual','maple','marble','march','margin','marine','market',
    'marriage','mask','mass','master','match','material','math','matrix','matter','maximum',
    'maze','meadow','mean','measure','meat','mechanic','medal','media','melody','melt',
    'member','memory','mention','menu','mercy','merge','merit','merry','mesh','message',
    'metal','method','middle','midnight','milk','million','mimic','mind','minimum','minor',
    'minute','miracle','mirror','misery','miss','mistake','mix','mixed','mixture','mobile',
    'model','modify','mom','moment','monitor','monkey','monster','month','moon','moral',
    'more','morning','mosquito','mother','motion','motor','mountain','mouse','move','movie',
    'much','muffin','mule','multiply','muscle','museum','mushroom','music','must','mutual',
    'myself','mystery','myth','naive','name','napkin','narrow','nasty','nation','nature',
    'near','neck','need','negative','neglect','neither','nephew','nerve','nest','net',
    'network','neutral','never','news','next','nice','night','noble','noise','nominee',
    'noodle','normal','north','nose','notable','note','nothing','notice','novel','now',
    'nuclear','number','nurse','nut','oak','obey','object','oblige','obscure','observe',
    'obtain','obvious','occur','ocean','october','odor','off','offer','office','often',
    'oil','okay','old','olive','olympic','omit','once','one','onion','online',
    'only','open','opera','opinion','oppose','option','orange','orbit','orchard','order',
    'ordinary','organ','orient','original','orphan','ostrich','other','outdoor','outer','output',
    'outside','oval','oven','over','own','owner','oxygen','oyster','ozone','pact',
    'paddle','page','pair','palace','palm','panda','panel','panic','panther','paper',
    'parade','parent','park','parrot','party','pass','patch','path','patient','patrol',
    'pattern','pause','pave','payment','peace','peanut','pear','peasant','pelican','pen',
    'penalty','pencil','people','pepper','perfect','permit','person','pet','phone','photo',
    'phrase','physical','piano','picnic','picture','piece','pig','pigeon','pill','pilot',
    'pink','pioneer','pipe','pistol','pitch','pizza','place','planet','plastic','plate',
    'play','please','pledge','pluck','plug','plunge','poem','poet','point','polar',
    'pole','police','pond','pony','pool','popular','portion','position','possible','post',
    'potato','pottery','poverty','powder','power','practice','praise','predict','prefer','prepare',
    'present','pretty','prevent','price','pride','primary','print','priority','prison','private',
    'prize','problem','process','produce','profit','program','project','promote','proof','property',
    'prosper','protect','proud','provide','public','pudding','pull','pulp','pulse','pumpkin',
    'punch','pupil','puppy','purchase','purity','purpose','purse','push','put','puzzle',
    'pyramid','quality','quantum','quarter','question','quick','quit','quiz','quote','rabbit',
    'raccoon','race','rack','radar','radio','rail','rain','raise','rally','ramp',
    'ranch','random','range','rapid','rare','rate','rather','raven','raw','razor',
    'ready','real','reason','rebel','rebuild','recall','receive','recipe','record','recycle',
    'reduce','reflect','reform','refuse','region','regret','regular','reject','relax','release',
    'relief','rely','remain','remember','remind','remove','render','renew','rent','reopen',
    'repair','repeat','replace','report','require','rescue','resemble','resist','resource','response',
    'result','retire','retreat','return','reunion','reveal','review','reward','rhythm','rib',
    'ribbon','rice','rich','ride','ridge','rifle','right','rigid','ring','riot',
    'ripple','risk','ritual','rival','river','road','roast','robot','robust','rocket',
    'romance','roof','rookie','room','rose','rotate','rough','round','route','royal',
    'rubber','rude','rug','rule','run','runway','rural','sad','saddle','sadness',
    'safe','sail','salad','salmon','salon','salt','salute','same','sample','sand',
    'satisfy','satoshi','sauce','sausage','save','say','scale','scan','scare','scatter',
    'scene','scheme','school','science','scissors','scorpion','scout','scrap','screen','script',
    'scrub','sea','search','season','seat','second','secret','section','security','seed',
    'seek','segment','select','sell','seminar','senior','sense','sentence','series','service',
    'session','settle','setup','seven','shadow','shaft','shallow','share','shed','shell',
    'sheriff','shield','shift','shine','ship','shiver','shock','shoe','shoot','shop',
    'short','shoulder','shove','shrimp','shrug','shuffle','shy','sibling','sick','side',
    'siege','sight','sign','silent','silk','silly','silver','similar','simple','since',
    'sing','siren','sister','situate','six','size','skate','sketch','ski','skill',
    'skin','skirt','skull','slab','slam','sleep','slender','slice','slide','slight',
    'slim','slogan','slot','slow','slush','small','smart','smile','smoke','smooth',
    'snack','snake','snap','sniff','snow','soap','soccer','social','sock','soda',
    'soft','solar','soldier','solid','solution','solve','someone','song','soon','sorry',
    'sort','soul','sound','soup','source','south','space','spare','spatial','spawn',
    'speak','special','speed','spell','spend','sphere','spice','spider','spike','spin',
    'spirit','split','spoil','sponsor','spoon','sport','spot','spray','spread','spring',
    'spy','square','squeeze','squirrel','stable','stadium','staff','stage','stairs','stamp',
    'stand','start','state','stay','steak','steel','stem','step','stereo','stick',
    'still','sting','stock','stomach','stone','stool','story','stove','strategy','street',
    'strike','strong','struggle','student','stuff','stumble','style','subject','submit','subway',
    'success','such','sudden','suffer','sugar','suggest','suit','summer','sun','sunny',
    'sunset','super','supply','supreme','sure','surface','surge','surprise','surround','survey',
    'suspect','sustain','swallow','swamp','swap','swarm','swear','sweet','swift','swim',
    'swing','switch','sword','symbol','symptom','syrup','system','table','tackle','tag',
    'tail','talent','talk','tank','tape','target','task','taste','tattoo','taxi',
    'teach','team','tell','ten','tenant','tennis','tent','term','test','text',
    'thank','that','theme','then','theory','there','they','thing','this','thought',
    'three','thrive','throw','thumb','thunder','ticket','tide','tiger','tilt','timber',
    'time','tiny','tip','tired','tissue','title','toast','tobacco','today','toddler',
    'toe','together','toilet','token','tomato','tomorrow','tone','tongue','tonight','tool',
    'tooth','top','topic','topple','torch','tornado','tortoise','toss','total','tourist',
    'toward','tower','town','toy','track','trade','traffic','tragic','train','transfer',
    'trap','trash','travel','tray','treat','tree','trend','trial','tribe','trick',
    'trigger','trim','trip','trophy','trouble','truck','true','truly','trumpet','trust',
    'truth','try','tube','tuition','tumble','tuna','tunnel','turkey','turn','turtle',
    'twelve','twenty','twice','twin','twist','two','type','typical','ugly','umbrella',
    'unable','unaware','uncle','uncover','under','undo','unfair','unfold','unhappy','uniform',
    'unique','unit','universe','unknown','unlock','until','unusual','unveil','update','upgrade',
    'uphold','upon','upper','upset','urban','urge','usage','use','used','useful',
    'useless','usual','utility','vacant','vacuum','vague','valid','valley','valve','van',
    'vanish','vapor','various','vast','vault','vehicle','velvet','vendor','venture','venue',
    'verb','verify','version','very','vessel','veteran','viable','vibrant','vicious','victory',
    'video','view','village','vintage','violin','virtual','virus','visa','visit','visual',
    'vital','vivid','vocal','voice','void','volcano','volume','vote','voyage','wage',
    'wagon','wait','walk','wall','walnut','want','warfare','warm','warrior','wash',
    'wasp','waste','water','wave','way','wealth','weapon','wear','weasel','weather',
    'web','wedding','weekend','weird','welcome','west','wet','whale','what','wheat',
    'wheel','when','where','whip','whisper','wide','width','wife','wild','will',
    'win','window','wine','wing','wink','winner','winter','wire','wisdom','wise',
    'wish','witness','wolf','woman','wonder','wood','wool','word','work','world',
    'worry','worth','wrap','wreck','wrestle','wrist','write','wrong','yard','year',
    'yellow','you','young','youth','zebra','zero','zone','zoo'
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

async function sha512(inputBytes) {
  const digest = await crypto.subtle.digest('SHA-512', inputBytes);
  return new Uint8Array(digest);
}

function base64UrlToBytes(value) {
  const padded = `${value}`.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(value.length / 4) * 4, '=');
  const raw = atob(padded);
  const output = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) {
    output[i] = raw.charCodeAt(i);
  }
  return output;
}

function seedToPkcs8(seedBytes) {
  if (!(seedBytes instanceof Uint8Array) || seedBytes.length !== 32) {
    throw new Error('Ed25519 seed must be 32 bytes');
  }

  const prefix = hexToBytes('302e020100300506032b657004220420');
  const pkcs8 = new Uint8Array(prefix.length + seedBytes.length);
  pkcs8.set(prefix, 0);
  pkcs8.set(seedBytes, prefix.length);
  return pkcs8;
}

async function deriveEd25519PublicKeyFromSeed(seedBytes) {
  const privateKey = await crypto.subtle.importKey(
    'pkcs8',
    seedToPkcs8(seedBytes),
    { name: 'Ed25519' },
    true,
    ['sign']
  );

  const jwk = await crypto.subtle.exportKey('jwk', privateKey);
  if (!jwk?.x) {
    throw new Error('Unable to derive Ed25519 public key');
  }

  return base64UrlToBytes(jwk.x);
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
  return words.length === 12 && words.every((word) => BIP39_WORDLIST.includes(word));
}

export async function mnemonicToKeypair(mnemonic) {
  const normalized = mnemonic.trim();
  const seedBytes = (await sha512(new TextEncoder().encode(normalized))).slice(0, 32);
  const privateKeyHex = bytesToHex(seedBytes);

  const publicKeyBytes = await deriveEd25519PublicKeyFromSeed(seedBytes);
  const publicKeyHex = bytesToHex(publicKeyBytes);
  const address = base58Encode(publicKeyBytes);

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

  const publicKeyBytes = await deriveEd25519PublicKeyFromSeed(seedBytes);
  return {
    privateKey: bytesToHex(seedBytes),
    publicKey: bytesToHex(publicKeyBytes),
    address: base58Encode(publicKeyBytes)
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
    [0,36,3,41,18],[1,44,10,45,2],[62,6,43,15,61],[28,55,25,21,56],[27,20,39,8,14]
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
      for (let x = 0; x < 5; x++) C[x] = state[x] ^ state[x+5] ^ state[x+10] ^ state[x+15] ^ state[x+20];
      for (let x = 0; x < 5; x++) {
        const D = C[(x+4)%5] ^ ((C[(x+1)%5] << 1n) | (C[(x+1)%5] >> 63n));
        for (let y = 0; y < 25; y += 5) state[y+x] ^= D;
      }
      // rho + pi
      const T = new BigUint64Array(25);
      for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) {
        const r = BigInt(ROT[x][y]);
        const v = state[y*5+x];
        T[x*5+((2*x+3*y)%5)] = r ? ((v << r) | (v >> (64n - r))) : v;
      }
      // chi
      for (let y = 0; y < 25; y += 5)
        for (let x = 0; x < 5; x++)
          state[y+x] = T[y+x] ^ ((~T[y+(x+1)%5]) & T[y+(x+2)%5]);
      // iota
      state[0] ^= RC[round];
    }
  }

  // Squeeze — 32 bytes
  const hash = new Uint8Array(32);
  for (let i = 0; i < 4; i++) {
    const v = state[i];
    for (let b = 0; b < 8; b++) hash[i*8+b] = Number((v >> BigInt(b*8)) & 0xFFn);
  }
  return Array.from(hash).map(b => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Derive EVM address from a MoltChain base58 address.
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
  if (!address || typeof address !== 'string') return false;
  try {
    const decoded = base58Decode(address.trim());
    return decoded.length === 32;
  } catch {
    return false;
  }
}

export async function signTransaction(privateKeyHex, messageBytes) {
  const seedBytes = hexToBytes(privateKeyHex);
  if (seedBytes.length !== 32) {
    throw new Error('Invalid Ed25519 seed length');
  }

  const privateKey = await crypto.subtle.importKey(
    'pkcs8',
    seedToPkcs8(seedBytes),
    { name: 'Ed25519' },
    false,
    ['sign']
  );

  const payload = messageBytes instanceof Uint8Array ? messageBytes : new Uint8Array(messageBytes);
  const signature = await crypto.subtle.sign('Ed25519', privateKey, payload);
  return new Uint8Array(signature);
}
