import Foundation

/// Ranks only the already compatible/granted targets; search never changes selection or authority.
enum ShareTargetSearch {
    static func filter(_ targets: [ShareTarget], query: String) -> [ShareTarget] {
        let terms = normalize(String(query.prefix(128))).split(whereSeparator: { $0.isWhitespace }).map(String.init)
        guard !terms.isEmpty else { return targets }
        return targets.enumerated().compactMap { index, target -> (ShareTarget, Int, Int)? in
            let fields = [target.capability.manifest.title, ServerPresentation.label(for: target.connection), target.capability.id].map(normalize)
            let scores = terms.compactMap { term in fields.compactMap { score(term, in: $0) }.max() }
            guard scores.count == terms.count else { return nil }
            return (target, scores.reduce(0, +), index)
        }.sorted { $0.1 == $1.1 ? $0.2 < $1.2 : $0.1 > $1.1 }.map { $0.0 }
    }

    private static func normalize(_ text: String) -> String {
        text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: Locale(identifier: "en_US_POSIX"))
    }

    private static func score(_ term: String, in field: String) -> Int? {
        let words = field.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).map(String.init)
        if words.contains(term) { return 120 }
        if field.hasPrefix(term) { return 110 }
        if field.contains(term) { return 100 }
        let needle = Array(term)
        var matched = 0
        var gaps = 0
        for character in field {
            if character == needle[matched] {
                matched += 1
                if matched == needle.count { return 70 - min(gaps, 30) }
            } else if matched > 0 { gaps += 1 }
        }
        guard needle.count >= 4 else { return nil }
        let limit = needle.count >= 7 ? 2 : 1
        let distance = words.filter { abs($0.count - needle.count) <= limit }
            .map { editDistance(needle, Array($0)) }.min()
        return distance.flatMap { $0 <= limit ? 40 - $0 * 10 : nil }
    }

    // Adjacent transpositions count as one typo ("slieds" → "slides").
    private static func editDistance(_ a: [Character], _ b: [Character]) -> Int {
        var rows = [Array(0...b.count)]
        for i in 1...a.count {
            var row = [i] + Array(repeating: 0, count: b.count)
            for j in 1...b.count {
                row[j] = min(row[j - 1] + 1, rows[i - 1][j] + 1, rows[i - 1][j - 1] + (a[i - 1] == b[j - 1] ? 0 : 1))
                if i > 1, j > 1, a[i - 1] == b[j - 2], a[i - 2] == b[j - 1] {
                    row[j] = min(row[j], rows[i - 2][j - 2] + 1)
                }
            }
            rows.append(row)
        }
        return rows[a.count][b.count]
    }
}
