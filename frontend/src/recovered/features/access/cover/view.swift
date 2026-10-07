import SwiftUI

struct AccessCoverView: View {
    let access: AccessCoverSandAccess
    let isVisible: Bool
    let onOpenAccess: @MainActor () -> Void

    var body: some View {
        if isVisible {
            let copy = AccessCoverModel.coverCopy(for: access)
            VStack(spacing: 18) {
                Spacer()
                Image(systemName: "person.3.sequence.fill")
                    .font(.system(size: 46, weight: .semibold))
                    .accessibilityHidden(true)
                Text("Fabushi")
                    .font(.largeTitle.bold())
                    .accessibilityIdentifier("fabushi-access-cover-heading")
                Text("Your team of always-on agents that finish the work.")
                    .font(.headline)
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
                VStack(alignment: .leading, spacing: 10) {
                    Text(copy.title).font(.title3.weight(.semibold))
                    Text(copy.body).font(.body).foregroundStyle(.secondary)
                    if let action = copy.action {
                        Button(action) { onOpenAccess() }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier("fabushi-access-cover-action")
                    }
                }
                .frame(maxWidth: 460, alignment: .leading)
                .padding(20)
                .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 18))
                Spacer()
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(28)
            .background(.background)
            .accessibilityIdentifier("fabushi-access-cover")
        }
    }
}
