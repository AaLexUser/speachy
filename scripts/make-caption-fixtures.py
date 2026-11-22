#!/usr/bin/env python3
"""Builds the fixture that holds caption timing to its measured accuracy.

    scripts/make-caption-fixtures.py MODELS RECOGNIZER OUT.json
    scripts/make-caption-fixtures.py --from RECORDINGS OUT.json

The fixture is read by `crates/speachy-core/tests/captions.rs`. For every recording it keeps
what `captions::words` is given and what it is scored against:

    shown    the sentence as written ("3,5 млрд рублей")
    spoken   the sentence as the model read it ("три с половиной миллиарда рублей")
    levels   the loudness of the trimmed audio, as `pipeline::loudness_over_time` computes it
    truth    for every whitespace token of `shown`, when a speech recognizer heard it start, in
             seconds; null for a token that is not scored

The first form makes the recordings: Supertonic 3 (MODELS is a Speachy models directory with
`supertonic-3` installed) reads each sentence below with voices 0 and 6, the audio is trimmed,
and the GigaAM v2 Russian recognizer (RECOGNIZER is the directory of
sherpa-onnx-nemo-ctc-giga-am-v2-russian-2025-04-19) gives each spoken word its start. A
recording is kept only if the recognizer heard the spoken text word for word. Needs the
`sherpa_onnx` and `numpy` Python packages and the models; it is not run in CI.

The second form reads recordings saved earlier as .npz files with `samples`, `rate` and a JSON
`meta` holding `shown`, `spoken` and `truth`.

A token is scored when it is the same word in `spoken`, or the first token with letters or
digits of a run that was rewritten ("3,5" of "3,5 млрд").
"""
import difflib, glob, json, os, re, sys
import numpy as np

WINDOW = 0.040   # pipeline.rs LEVEL_WINDOW
VOICES = [0, 6]

# (shown, spoken); spoken is None where the model reads the shown text.
SENTENCES = [
    ('Учёные обнаружили, что регулярные прогулки на свежем воздухе заметно улучшают память и внимание.', None),
    ('Правительство объявило о новых мерах поддержки малого бизнеса в регионах.', None),
    ('Компания представила обновлённую версию своего приложения с расширенными возможностями поиска.', None),
    ('По словам экспертов, цены на жильё в крупных городах продолжат расти до конца года.', None),
    ('Исследователи считают, что искусственный интеллект изменит рынок труда быстрее, чем ожидалось.', None),
    ('В столице открылась выставка современного искусства, которая продлится до середины весны.', None),
    ('Синоптики предупреждают о сильном ветре и мокром снеге в ближайшие выходные.', None),
    ('Новый закон вступит в силу после того, как его подпишет президент.', None),
    ('Разработчики исправили ошибку, из-за которой программа неожиданно закрывалась при запуске.', None),
    ('Многие читатели предпочитают слушать длинные статьи по дороге на работу.', None),
    ('Замок на двери старого замка давно заржавел, и открыть его было непросто.', None),
    ('Большая часть населения страны живёт в городах на берегах крупных рек.', None),
    ('Когда дождь закончился, дети выбежали во двор, где уже блестели лужи, и стали пускать кораблики.', None),
    ('Он долго молчал; потом сказал тихо: всё будет хорошо, только не торопись.', None),
    ('Учёные — физики, химики и биологи — собрались в большом зале, чтобы обсудить результаты.', None),
    ('Если поезд опоздает, мы пропустим пересадку, а следующий рейс будет только утром.', None),
    ('Выручка компании выросла на 15% и достигла 3,5 млрд рублей.', 'Выручка компании выросла на пятнадцать процентов и достигла три с половиной миллиарда рублей.'),
    ('По данным ВОЗ, около 40 млн человек нуждаются в помощи.', 'По данным ВОЗ, около сорок миллионов человек нуждаются в помощи.'),
    ('Температура опустится до −20 °C, а скорость ветра составит 15 м/с.', 'Температура опустится до минус двадцать градусов Цельсия, а скорость ветра составит пятнадцать метров в секунду.'),
    ('Поезд разгоняется до 250 км/ч и проходит 650 км за 4 ч.', 'Поезд разгоняется до двести пятьдесят километров в час и проходит шестьсот пятьдесят километров за четыре часа.'),
    ('Цена выросла с 1,2 тыс. до 2,75 тыс. рублей, т. е. более чем вдвое.', 'Цена выросла с одна целая две десятых тысячи до две целых семьдесят пять сотых тысячи рублей, то есть более чем вдвое.'),
    ('В магазине продаются фрукты, овощи, хлеб и т. д., а также напитки, соки и т. п.', 'В магазине продаются фрукты, овощи, хлеб и так далее, а также напитки, соки и тому подобное.'),
    ('Офис находится по адресу: ул. Ленина, д. 5, кв. 12.', 'Офис находится по адресу: улица Ленина, дом пять, квартира двенадцать.'),
    ('Закон № 152 вступил в силу в 2006 г.', 'Закон номер сто пятьдесят два вступил в силу в две тысячи шесть году.'),
    ('Смартфон стоит $999, или около 90 тыс. ₽.', 'Смартфон стоит девятьсот девяносто девять долларов, или около девяносто тысяч рублей.'),
    ('Вес посылки не должен превышать 2,5 кг, а длина — 60 см.', 'Вес посылки не должен превышать два с половиной килограмма, а длина — шестьдесят сантиметров.'),
    ('Инфляция замедлилась до 4,3%, сообщили в министерстве.', 'Инфляция замедлилась до четыре целых три десятых процента, сообщили в министерстве.'),
    ('Население города составляет 1,4 млн чел., из них 52% — женщины.', 'Население города составляет одна целая четыре десятых миллиона человек, из них пятьдесят два процента — женщины.'),
    ('Проект оценивается в €1,5 млрд и займёт 4 года.', 'Проект оценивается в полтора миллиарда евро и займёт четыре года.'),
]


def key(word):
    return re.sub(r"[^а-яa-z0-9]", "", word.lower().replace("ё", "е"))


def loudness_over_time(samples, rate):
    """pipeline.rs `loudness_over_time`."""
    size = max(1, int(rate * WINDOW))
    strength = [np.sqrt(np.mean(np.square(samples[i:i + size], dtype=np.float64)))
                for i in range(0, len(samples), size)]
    loudest = max(strength, default=0.0)
    if loudest < 1e-6:
        return [0] * len(strength)
    return [int(round((s / loudest) ** 0.6 * 100)) for s in strength]


def truth(shown, spoken, starts):
    """The recognizer's start for every scored token of `shown`."""
    tokens = shown.split()
    a = [key(t) for t in tokens]
    b = [key(t) for t in spoken.split() if key(t)]
    out = [None] * len(tokens)
    for tag, i0, i1, j0, j1 in difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes():
        if tag == "equal":
            for k in range(i1 - i0):
                if a[i0 + k]:
                    out[i0 + k] = starts[j0 + k]
        elif i1 > i0 and j1 > j0:
            first = next((i for i in range(i0, i1) if a[i]), None)
            if first is not None:
                out[first] = starts[j0]
    return out


def record(models, recognizer):
    """Synthesizes and recognizes every sentence: (id, shown, spoken, samples, rate, truth)."""
    import sherpa_onnx
    d = os.path.join(models, "supertonic-3")
    tts = sherpa_onnx.OfflineTts(sherpa_onnx.OfflineTtsConfig(model=sherpa_onnx.OfflineTtsModelConfig(
        supertonic=sherpa_onnx.OfflineTtsSupertonicModelConfig(
            duration_predictor=f"{d}/duration_predictor.int8.onnx", text_encoder=f"{d}/text_encoder.int8.onnx",
            vector_estimator=f"{d}/vector_estimator.int8.onnx", vocoder=f"{d}/vocoder.int8.onnx",
            tts_json=f"{d}/tts.json", unicode_indexer=f"{d}/unicode_indexer.bin", voice_style=f"{d}/voice.bin"),
        num_threads=4), max_num_sentences=1))
    rec = sherpa_onnx.OfflineRecognizer.from_nemo_ctc(
        model=f"{recognizer}/model.int8.onnx", tokens=f"{recognizer}/tokens.txt", num_threads=4)

    def heard(samples, rate):
        stream = rec.create_stream()
        stream.accept_waveform(rate, samples)
        rec.decode_stream(stream)
        words, starts, current = [], [], ""
        for token, at in zip(stream.result.tokens, stream.result.timestamps):
            if token.strip() == "":
                if current:
                    words.append(current)
                    current = ""
                continue
            if not current:
                starts.append(float(at))
            current += token
        if current:
            words.append(current)
        return [key(w) for w in words], starts

    def trim(samples):
        loud = np.flatnonzero(np.abs(samples) > 0.01)
        return samples[loud[0]: loud[-1] + 1] if len(loud) else samples

    dropped = 0
    for n, (shown, spoken) in enumerate(SENTENCES):
        spoken = spoken or shown
        for voice in VOICES:
            config = sherpa_onnx.GenerationConfig()
            config.sid, config.speed, config.extra = voice, 1.0, {"lang": "ru"}
            audio = tts.generate(spoken, config)
            samples = trim(np.array(audio.samples, dtype=np.float32))
            words, starts = heard(samples, audio.sample_rate)
            if words != [key(w) for w in spoken.split() if key(w)]:
                dropped += 1
                continue
            yield f"{n:02d}-v{voice}", shown, spoken, samples, audio.sample_rate, truth(shown, spoken, starts)
    print(f"{dropped} recordings dropped: the recognizer heard other words", file=sys.stderr)


def saved(directory):
    for path in sorted(glob.glob(os.path.join(directory, "*.npz"))):
        z = np.load(path, allow_pickle=False)
        meta = json.loads(str(z["meta"]))
        yield (os.path.basename(path)[:-4], meta["shown"], meta["spoken"], z["samples"], int(z["rate"]),
               meta["truth"])


def main(args):
    if len(args) == 3 and args[0] == "--from":
        recordings, out = saved(args[1]), args[2]
    elif len(args) == 3:
        recordings, out = record(args[0], args[1]), args[2]
    else:
        sys.exit(__doc__)
    fixture = [dict(id=id, shown=shown, spoken=spoken, levels=loudness_over_time(samples, rate), truth=truth)
               for id, shown, spoken, samples, rate, truth in recordings]
    with open(out, "w", encoding="utf-8") as f:
        f.write("[\n" + ",\n".join(json.dumps(item, ensure_ascii=False, separators=(",", ":")) for item in fixture) + "\n]\n")
    print(f"{len(fixture)} recordings, {sum(t is not None for item in fixture for t in item['truth'])} scored words",
          file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1:])
