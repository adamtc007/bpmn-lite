import json, sys, torch
sys.path.insert(0, '/Users/adamtc007/dev/bpmn-lite/utterance-engine/train_py')
import train_slm
from safetensors.torch import load_file
from transformers import AutoTokenizer, AutoModel
from collections import defaultdict

corpus_path, weights_path = sys.argv[1], sys.argv[2]
cfg = train_slm.BASES['modernbert-base']
device = 'mps' if torch.backends.mps.is_available() else 'cpu'
tokenizer = AutoTokenizer.from_pretrained(str(train_slm.BUNDLE_DIR / 'modernbert-base'))
encoder = AutoModel.from_pretrained(cfg['repo'], revision=cfg['revision'])
model = train_slm.ListwiseReranker(encoder, encoder.config.hidden_size, train_slm.POOLING['modernbert-base'])
state = load_file(weights_path)
model.encoder.load_state_dict({k[8:]: v for k, v in state.items() if k.startswith('encoder.')})
model.head.load_state_dict({k[5:]: v for k, v in state.items() if k.startswith('head.')})
model = model.to(device).eval()

records = [json.loads(l) for l in open(corpus_path)]
fam = json.load(open(train_slm.SPLIT_MANIFEST_PATH))['family_split']
test = [r for r in records if fam[r['family_id']] == 'test']
correct, per_class = 0, defaultdict(lambda: [0, 0])
with torch.no_grad():
    for r in test:
        enc, label_idx = train_slm.encode_example(tokenizer, r, device)
        ok = int(model(**enc).squeeze(-1).argmax()) == label_idx
        cls = r['family_id'].split('::')[0]
        per_class[cls][1] += 1
        if ok: correct += 1; per_class[cls][0] += 1
out = {"top1": correct/len(test), "n": len(test),
       "per_class": {k: {"correct": v[0], "total": v[1]} for k, v in sorted(per_class.items())}}
print(json.dumps(out))
