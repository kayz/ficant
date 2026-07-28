Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The fixture server is a live gRPC/gRPC-Web composition of the real API, native engines and
# CGB RulePack parser.  Its one immutable RulePack is supplied by a test-only repository so this
# offline SDK parity check does not turn the ordinary local gate into a PostgreSQL/Ceph topology.
# The separate R2 topology/SIT evidence exercises the persisted production repository.
$arguments = @(
    'test',
    '--offline',
    '--locked',
    '-p',
    'ficant-api',
    '--test',
    'phase2e_sdk_live',
    '--',
    '--ignored',
    '--exact',
    'python_sdk_matches_phase2_reference_slices_through_live_rule_pack_composition'
)
& cargo @arguments
exit $LASTEXITCODE
