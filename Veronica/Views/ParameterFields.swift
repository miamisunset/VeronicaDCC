import SwiftUI

/// Generic, `ParamValue`-driven numeric editor components for the parameter
/// editor.
///
/// Both fields are case-keyed, never operator-keyed: the float field edits
/// any `.float` parameter and the triple-field any `.vec3` parameter, so
/// future operators reuse them with no drift. Draft text lives in the TCA
/// store (these views only report keystrokes); validation runs through
/// `NumericDraftParsing`, the same parser the reducer commits through, so
/// non-numeric input can never commit. Failed validation shows an inline
/// hint; the commit callbacks are still sent on submit/focus-loss and the
/// reducer no-ops them when invalid (defense in depth).
struct FloatParamField: View {
    /// Parameter key (accessibility identifier + validation scope).
    let key: String
    /// Current draft text, owned by the store.
    let text: String
    /// Reports keystrokes to the store.
    let onChanged: (String) -> Void
    /// Requests a commit (submit or focus loss). The reducer no-ops when
    /// the draft does not parse.
    let onCommitted: () -> Void
    /// Requests a revert (Escape).
    let onReverted: () -> Void

    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(key)
                    .font(.callout)
                Spacer()
                TextField(
                    key,
                    text: Binding(
                        get: { text },
                        set: { onChanged($0) }
                    )
                )
                .textFieldStyle(.roundedBorder)
                .multilineTextAlignment(.trailing)
                .frame(width: 120)
                .focused($focused)
                .onSubmit {
                    onCommitted()
                }
                .onExitCommand {
                    onReverted()
                }
                .accessibilityIdentifier("parameterField-\(key)")
                .accessibilityLabel("\(key) value")
                Text(ParameterValue.float(0).typeLabel)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if !isValid {
                Text("Enter a number.")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .accessibilityIdentifier("parameterField-\(key)-error")
            }
        }
        .onChange(of: focused) {
            if !focused {
                onCommitted()
            }
        }
    }

    /// Field-level validity: the single source is the commit parser.
    private var isValid: Bool {
        NumericDraftParsing.parseFloatDraft(text) != nil
    }
}

/// Generic triple-field for any `.vec3` parameter (size, center, offsets).
///
/// The triple commits as a unit: tabbing between the three axes never
/// commits (focus only leaves the group as a whole), and one non-numeric
/// component vetoes the whole commit — never partially. (Return in one
/// axis also submits immediately with the other axes as they stand, so
/// "unit" means the triple validates together, not that all three were
/// just edited.)
struct Vec3ParamField: View {
    /// Parameter key (accessibility identifiers + validation scope).
    let key: String
    /// Current component drafts (exactly 3), owned by the store.
    let components: [String]
    /// Reports keystrokes to the store with the axis index (0-2).
    let onChanged: (Int, String) -> Void
    /// Requests a unit commit. The reducer no-ops when any component does
    /// not parse.
    let onCommitted: () -> Void
    /// Requests a revert (Escape).
    let onReverted: () -> Void

    /// Axis labels for the three components.
    static let axisLabels = ["X", "Y", "Z"]

    @FocusState private var focusedAxis: Int?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(key)
                    .font(.callout)
                Spacer()
                ForEach(0..<3, id: \.self) { axis in
                    TextField(
                        Self.axisLabels[axis],
                        text: Binding(
                            get: { components.count > axis ? components[axis] : "" },
                            set: { onChanged(axis, $0) }
                        )
                    )
                    .textFieldStyle(.roundedBorder)
                    .multilineTextAlignment(.trailing)
                    .frame(width: 72)
                    .focused($focusedAxis, equals: axis)
                    .onSubmit {
                        onCommitted()
                    }
                    .onExitCommand {
                        onReverted()
                    }
                    .accessibilityIdentifier("parameterField-\(key)-\(axis)")
                    .accessibilityLabel("\(key) \(Self.axisLabels[axis])")
                }
                Text(ParameterValue.vec3(0, 0, 0).typeLabel)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if !isValid {
                Text("Enter numbers for X, Y and Z.")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .accessibilityIdentifier("parameterField-\(key)-error")
            }
        }
        .onChange(of: focusedAxis) {
            // Commits only when focus leaves the group as a whole: moving
            // between axes keeps editing the same uncommitted triple.
            if focusedAxis == nil {
                onCommitted()
            }
        }
    }

    /// Triple-level validity: the single source is the commit parser.
    private var isValid: Bool {
        NumericDraftParsing.parseVec3Draft(components) != nil
    }
}

#Preview("Float field") {
    @Previewable @State var draft = "2.5"
    FloatParamField(
        key: "gain",
        text: draft,
        onChanged: { draft = $0 },
        onCommitted: {},
        onReverted: { draft = "2.5" }
    )
    .padding()
}

#Preview("Float field invalid") {
    FloatParamField(
        key: "gain",
        text: "oops",
        onChanged: { _ in },
        onCommitted: {},
        onReverted: {}
    )
    .padding()
}

#Preview("Triple field") {
    @Previewable @State var components = ["1", "1", "1"]
    Vec3ParamField(
        key: "size",
        components: components,
        onChanged: { axis, value in components[axis] = value },
        onCommitted: {},
        onReverted: { components = ["1", "1", "1"] }
    )
    .padding()
}

#Preview("Triple field invalid") {
    Vec3ParamField(
        key: "size",
        components: ["2", "oops", "1"],
        onChanged: { _, _ in },
        onCommitted: {},
        onReverted: {}
    )
    .padding()
}
