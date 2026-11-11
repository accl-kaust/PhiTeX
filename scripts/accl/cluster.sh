# The cluster the drivers (accl, gate-fast, corpus-sweep) talk to,
# sourced by them: CLUSTER=accl (the default, unset) or CLUSTER=mc.
#
#   H        the login host, reached as `ssh $H`
#   remote   this repository's git remote for that host's bare repo
#            (accl: acclhead1:code/flinner/phitex.git; mc: `git remote
#            add mc mcmgt01:code/flinner/phitex.git`)
#   part     the sbatch options that pick where a job runs
#   light    the same for a small job (ci-discover)
#   ws_node  the node of a kept workspace (ACCL_WS) unless ACCL_WS_NODE
#   gate_x   gate-fast's default GATE_SBATCH
#   gate_ctl gate-fast's default GATE_CONTROL (its timings recorded on accl)
#   perf_node the node ci-oracle keeps free (CI_PERF_NODE), none on mc
#   runs     the local copy of the results (job ids are per cluster)
#
# mc (`ssh mcmgt01`, account pi-fahmysa, QOS normal): partition mc only
# (synode refuses the account); its CPU-only nodes, mcnode42 and 44 (256
# CPUs, 1.5 TB each, feature cpu_amd_epyc_9754), chosen by the
# constraint, never the GPU nodes. Slurm is on the default PATH ($slurm's
# /opt/slurm/current/bin is absent there and harmless). The nodes have no
# Apptainer: the job scripts use ~/shared-cache/apptainer (official
# release, user space, apptainer-user.sbatch). The same paths under the
# home as on accl: code/flinner/phitex.git, phitex-accl, phitex-runs,
# phitex-ci, shared-cache/phitex (the images, copied from accl).
export CLUSTER=${CLUSTER:-accl}
case $CLUSTER in
  accl)
    # acclhead1 lost its NFS home (2026-11-03): only its sacct is used (a
    # node's sacct cannot reach slurmdbd); a node stages and submits.
    H=acclnode03 SH=acclhead1 remote=accl-nfs part="-p build --qos=build4" light="-p light"
    ws_node=acclnode01 gate_x=--exclude=harrier perf_node=acclnode04 gate_ctl=566fb842 gate_one=0
    runs=$HOME/code/flinner/phitex-runs ;;
  mc)
    # (MC_FEATURE: another node feature, or empty for any mc node)
    mcf=${MC_FEATURE-cpu_amd_epyc_9754}; mcf=${mcf:+-C $mcf}
    H=mcmgt01 SH=mcmgt01 remote=mc part="-p mc $mcf" light="-p mc $mcf"
    ws_node=mcnode42 gate_x= perf_node= gate_ctl= gate_one=1
    runs=$HOME/code/flinner/phitex-runs/mc ;;
  *) echo "CLUSTER=$CLUSTER: accl or mc" >&2; exit 2 ;;
esac
