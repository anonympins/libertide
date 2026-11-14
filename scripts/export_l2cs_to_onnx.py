#!/usr/bin/env python3
"""
Script d'export de L2CS-Net (safetensors -> ONNX) pour Libertide.
Compatible avec src/eyetracking.rs.

Prérequis :
    pip install torch torchvision safetensors onnx

Exemple d'utilisation :
    python scripts/export_l2cs_to_onnx.py --safetensors l2cs_gaze360_resnet50.safetensors
"""

import argparse
import sys
from pathlib import Path

import torch
import torch.nn as nn
from torchvision.models.resnet import Bottleneck, ResNet

try:
    from safetensors.torch import load_file as load_safetensors
except ImportError:
    print("Erreur: safetensors n'est pas installé. Exécutez : pip install safetensors")
    sys.exit(1)


class L2CSNet(ResNet):
    """
    Architecture L2CS-Net basée sur ResNet-50 avec deux têtes FC parallèles
    de 90 classes (bins de 4° couvrant [-180°, +180°]) pour yaw et pitch.
    """

    def __init__(self, block=Bottleneck, layers=[3, 4, 6, 3], num_bins=90):
        super(L2CSNet, self).__init__(block, layers)
        self.num_bins = num_bins
        in_features = 512 * block.expansion  # 2048 pour ResNet-50

        # Suppression de la tête FC par défaut
        self.fc = nn.Identity()

        # Têtes jumelles d'estimation de direction du regard
        self.fc_yaw = nn.Linear(in_features, num_bins)
        self.fc_pitch = nn.Linear(in_features, num_bins)

    def forward(self, x):
        x = self.conv1(x)
        x = self.bn1(x)
        x = self.relu(x)
        x = self.maxpool(x)

        x = self.layer1(x)
        x = self.layer2(x)
        x = self.layer3(x)
        x = self.layer4(x)

        x = self.avgpool(x)
        feat = torch.flatten(x, 1)

        yaw = self.fc_yaw(feat)
        pitch = self.fc_pitch(feat)
        return yaw, pitch


class L2CSNetContinuous(nn.Module):
    """
    Wrapper optionnel calculant l'espérance Softmax directement dans le graphe ONNX
    pour sortir [yaw_deg, pitch_deg] sous forme scalaire.
    """

    def __init__(self, base_model: L2CSNet):
        super().__init__()
        self.base_model = base_model
        self.softmax = nn.Softmax(dim=1)
        # Bins de 0 à 89 (4°/bin -> -180° à +180°)
        self.register_buffer("idx_tensor", torch.arange(90, dtype=torch.float32))

    def forward(self, x):
        yaw_logits, pitch_logits = self.base_model(x)
        yaw_prob = self.softmax(yaw_logits)
        pitch_prob = self.softmax(pitch_logits)

        yaw_deg = torch.sum(yaw_prob * self.idx_tensor, dim=1, keepdim=True) * 4.0 - 180.0
        pitch_deg = torch.sum(pitch_prob * self.idx_tensor, dim=1, keepdim=True) * 4.0 - 180.0
        return yaw_deg, pitch_deg


def export_to_onnx(safetensors_path: str, output_path: str, continuous: bool = False, opset_version: int = 18):
    print(f"[1/4] Chargement des poids depuis {safetensors_path}...")
    state_dict = load_safetensors(safetensors_path)

    # Nettoyage des préfixes éventuels (ex: 'model.', 'module.')
    cleaned_state_dict = {}
    for k, v in state_dict.items():
        new_k = k
        for prefix in ["model.", "module.", "base_model."]:
            if new_k.startswith(prefix):
                new_k = new_k[len(prefix):]
        # Remapping automatique pour les checkpoints Py-Feat / Hugging Face
        new_k = new_k.replace("fc_yaw_gaze.", "fc_yaw.")
        new_k = new_k.replace("fc_pitch_gaze.", "fc_pitch.")
        cleaned_state_dict[new_k] = v

    # Vérification d'erreur bloquante sur les têtes de regard
    required_head_keys = ["fc_yaw.weight", "fc_yaw.bias", "fc_pitch.weight", "fc_pitch.bias"]
    missing_heads = [k for k in required_head_keys if k not in cleaned_state_dict]
    if missing_heads:
        print(f"Erreur critique: Les poids des têtes de regard sont introuvables : {missing_heads}")
        print("Vérifiez la structure des clés du fichier safetensors.")
        sys.exit(1)

    print("[2/4] Instanciation du modèle L2CS-Net ResNet-50...")
    model = L2CSNet(block=Bottleneck, layers=[3, 4, 6, 3], num_bins=90)
    missing, unexpected = model.load_state_dict(cleaned_state_dict, strict=False)

    if missing:
        print(f"  [Info] Clés manquantes non critiques : {missing[:5]}")
    if unexpected:
        print(f"  [Info] Clés ignorées : {unexpected[:5]}")

    model.eval()

    export_model = L2CSNetContinuous(model) if continuous else model
    export_model.eval()

    # Tenseur dummy NCHW statique [1, 3, 224, 224]
    dummy_input = torch.randn(1, 3, 224, 224, dtype=torch.float32)

    # Gestion robuste du répertoire de destination
    out_file = Path(output_path).resolve()
    out_file.parent.mkdir(parents=True, exist_ok=True)

    print(f"[3/4] Export ONNX vers {out_file} (opset={opset_version}, continuous={continuous}, static=[1, 3, 224, 224])...")
    export_kwargs = {
        "export_params": True,
        "opset_version": opset_version,
        "do_constant_folding": True,
        "input_names": ["input"],
        "output_names": ["yaw", "pitch"],
    }

    # dynamo=False contourne le bogue d'export TorchScript/onnxscript sur PyTorch 2.x / Python 3.12+
    try:
        torch.onnx.export(
            export_model,
            dummy_input,
            str(out_file),
            dynamo=False,
            **export_kwargs,
        )
    except TypeError:
        torch.onnx.export(
            export_model,
            dummy_input,
            str(out_file),
            **export_kwargs,
        )

    print("[4/4] Validation du modèle ONNX...")
    try:
        import onnx
        onnx_model = onnx.load(output_path)
        onnx.checker.check_model(onnx_model)
        print(" Modèle ONNX validé avec succès !")
    except ImportError:
        print("  [Note] Bibliothèque 'onnx' non installée pour la vérification (pip install onnx).")
    except Exception as e:
        print(f"  [Attention] Erreur de vérification ONNX : {e}")

    print(f"\nTerminé ! Déposez le fichier dans models/eyetracking/L2CSNet_gaze.onnx")


def main():
    parser = argparse.ArgumentParser(description="Export L2CS-Net safetensors vers ONNX pour libertide.")
    parser.add_argument(
        "-s", "--safetensors",
        type=str,
        required=True,
        help="Chemin vers le fichier safetensors (ex: l2cs_gaze360_resnet50.safetensors)",
    )
    parser.add_argument(
        "-o", "--output",
        type=str,
        default="models/eyetracking/L2CSNet_gaze.onnx",
        help="Fichier ONNX de sortie (défaut: models/eyetracking/L2CSNet_gaze.onnx)",
    )
    parser.add_argument(
        "--continuous",
        action="store_true",
        help="Calcule l'espérance Softmax dans l'ONNX (sorties scalaires [1, 1] en degrés au lieu des 90 logits)",
    )
    parser.add_argument(
        "--opset",
        type=int,
        default=18,
        help="Version opset ONNX (défaut: 18)",
    )

    args = parser.parse_args()
    export_to_onnx(args.safetensors, args.output, continuous=args.continuous, opset_version=args.opset)


if __name__ == "__main__":
    main()