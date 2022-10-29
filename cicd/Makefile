DO_PAT := $(shell cat ~/.config/doctl/config.yaml | yq e '.access-token' -)
GITLAB_REG_KEY := $(shell sops -d secrets.yaml | yq e '.gitlab-reg-key' -)
GITLAB_SERVER_URL := $(shell sops -d secrets.yaml | yq e '.gitlab-server-url' -)
RUNNER_NAME := "veilid-runner-1"
KEYNAME := "pensfabriko"


plan-runner:
	terraform plan \
		-var "do_token=${DO_PAT}" \
		-var "pvt_key=${HOME}/.ssh/id_rsa" \
		-var "ssh_key=${KEYNAME}" \
		-var "reg_key=${GITLAB_REG_KEY}" \
		-var "ci_server_url=${GITLAB_SERVER_URL}" \
		-var "runner_name=${RUNNER_NAME}"

create-runner:
	terraform apply \
		-var "do_token=${DO_PAT}" \
		-var "pvt_key=${HOME}/.ssh/id_rsa" \
		-var "ssh_key=${KEYNAME}" \
		-var "reg_key=${GITLAB_REG_KEY}" \
		-var "ci_server_url=${GITLAB_SERVER_URL}" \
		-var "runner_name=${RUNNER_NAME}"

destroy-runner:
	terraform destroy \
		-var "do_token=${DO_PAT}" \
		-var "pvt_key=${HOME}/.ssh/id_rsa" \
		-var "ssh_key=${KEYNAME}" \
		-var "reg_key=${GITLAB_REG_KEY}" \
		-var "ci_server_url=${GITLAB_SERVER_URL}" \
		-var "runner_name=${RUNNER_NAME}"
